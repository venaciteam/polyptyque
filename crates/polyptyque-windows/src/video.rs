//! Capture d'une fenêtre par Windows.Graphics.Capture.
//!
//! Chaque nouvelle image est copiée, mise à l'échelle (proportions
//! conservées, bandes noires) et convertie en NV12 BT.709 par le processeur
//! vidéo de la carte graphique, puis relue en mémoire pour l'encodeur. Les
//! images ne sont traitées que lorsque le contenu change.

use std::mem::ManuallyDrop;
use std::sync::{Arc, Once};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use polyptyque_core::platform::{Nv12Frame, VideoCapture, WindowHandle};
use polyptyque_core::video::fit_rect;
use windows::core::Interface;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureAccess, GraphicsCaptureAccessKind,
    GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, RECT, TRUE};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D, ID3D11VideoContext,
    ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView,
    ID3D11VideoProcessorOutputView, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
    D3D11_SDK_VERSION, D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, D3D11_VIDEO_COLOR, D3D11_VIDEO_COLOR_0, D3D11_VIDEO_COLOR_RGBA,
    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_COLOR_SPACE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

use crate::windows_list::hwnd;

const PIXEL_FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;
const POOL_BUFFERS: i32 = 2;
/// Espace colorimétrique d'entrée : RVB pleine plage (0-255).
const INPUT_COLOR_SPACE: u32 = 0;
/// Espace colorimétrique de sortie : YCbCr BT.709, plage « TV » 16-235.
/// Champs de bits : Usage (bit 0), RGB_Range (1), YCbCr_Matrix (2),
/// YCbCr_xvYCC (3), Nominal_Range (4-5).
const OUTPUT_COLOR_SPACE: u32 = (1 << 2) | (1 << 4);

pub struct WgcCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    winrt_device: IDirect3DDevice,
    _item: GraphicsCaptureItem,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    pool_size: SizeInt32,
    out_w: u32,
    out_h: u32,
    output: ID3D11Texture2D,
    staging: ID3D11Texture2D,
    processor: Option<Processor>,
}

/// Processeur vidéo, propre à une taille d'entrée.
struct Processor {
    in_w: u32,
    in_h: u32,
    processor: ID3D11VideoProcessor,
    input: ID3D11Texture2D,
    input_view: ID3D11VideoProcessorInputView,
    output_view: ID3D11VideoProcessorOutputView,
    _enumerator: ID3D11VideoProcessorEnumerator,
}

impl WgcCapture {
    pub fn open(handle: WindowHandle, out_w: u32, out_h: u32, capture_cursor: bool) -> Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
            bail!(tr::tr!("la capture de fenêtre (Windows.Graphics.Capture) n'est pas disponible sur ce système"));
        }
        let (device, context) = create_device()?;
        // La capture Windows écrit dans les surfaces depuis ses propres fils
        // pendant que ce fil convertit les images : on protège le contexte.
        unsafe {
            let _ = context.cast::<ID3D11Multithread>()?.SetMultithreadProtected(true);
        }
        let dxgi: IDXGIDevice = device.cast()?;
        let winrt_device: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;

        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem =
            unsafe { interop.CreateForWindow(hwnd(handle)) }.context(tr::tr!("capture de la fenêtre refusée"))?;
        let size = item.Size()?;
        let pool_size = SizeInt32 { Width: size.Width.max(16), Height: size.Height.max(16) };
        let pool =
            Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt_device, PIXEL_FORMAT, POOL_BUFFERS, pool_size)?;
        let session = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsCursorCaptureEnabled(capture_cursor);
        // Menus déroulants et infobulles (Windows 11 24H2 et plus récents).
        let _ = session.SetIncludeSecondaryWindows(true);
        // Retire la bordure jaune quand Windows le permet (Windows 11).
        request_borderless_access();
        if let Err(e) = session.SetIsBorderRequired(false) {
            log::debug!("bordure de capture non désactivable : {e}");
        }

        let video_device: ID3D11VideoDevice =
            device.cast().context(tr::tr!("le pilote graphique ne fournit pas le traitement vidéo D3D11"))?;
        let video_context: ID3D11VideoContext = context.cast()?;
        let output = create_texture(
            &device,
            out_w,
            out_h,
            DXGI_FORMAT_NV12,
            D3D11_USAGE_DEFAULT,
            D3D11_BIND_RENDER_TARGET.0 as u32,
            0,
        )?;
        let staging = create_texture(
            &device,
            out_w,
            out_h,
            DXGI_FORMAT_NV12,
            D3D11_USAGE_STAGING,
            0,
            D3D11_CPU_ACCESS_READ.0 as u32,
        )?;
        session.StartCapture()?;

        Ok(Self {
            device,
            context,
            video_device,
            video_context,
            winrt_device,
            _item: item,
            pool,
            session,
            pool_size,
            out_w,
            out_h,
            output,
            staging,
            processor: None,
        })
    }

    fn process(&mut self, frame: &Direct3D11CaptureFrame) -> Result<Nv12Frame> {
        let content = frame.ContentSize()?;
        let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
        let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };
        self.ensure_processor(desc.Width, desc.Height)?;
        let state = self.processor.as_ref().expect("processeur vidéo initialisé");

        // Partie utile de la surface (elle peut être plus grande que la
        // fenêtre juste après un redimensionnement).
        let src_w = (content.Width.max(2) as u32).min(desc.Width);
        let src_h = (content.Height.max(2) as u32).min(desc.Height);
        let (x, y, w, h) = fit_rect(src_w, src_h, self.out_w, self.out_h);
        let source = RECT { left: 0, top: 0, right: src_w as i32, bottom: src_h as i32 };
        let dest = RECT { left: x as i32, top: y as i32, right: (x + w) as i32, bottom: (y + h) as i32 };

        unsafe {
            self.context.CopyResource(&state.input, &texture);
            self.video_context.VideoProcessorSetStreamSourceRect(&state.processor, 0, true, Some(&source));
            self.video_context.VideoProcessorSetStreamDestRect(&state.processor, 0, true, Some(&dest));
            let streams = [D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: TRUE,
                pInputSurface: ManuallyDrop::new(Some(state.input_view.clone())),
                ..Default::default()
            }];
            let blit = self.video_context.VideoProcessorBlt(&state.processor, &state.output_view, 0, &streams);
            // Libère la référence prise pour l'appel.
            for stream in streams {
                drop(ManuallyDrop::into_inner(stream.pInputSurface));
            }
            blit.context(tr::tr!("conversion de l'image par la carte graphique"))?;

            self.context.CopyResource(&self.staging, &self.output);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .context(tr::tr!("lecture de l'image convertie"))?;
            let data = copy_nv12(&mapped, self.out_w as usize, self.out_h as usize);
            self.context.Unmap(&self.staging, 0);
            Ok(Arc::new(data))
        }
    }

    fn ensure_processor(&mut self, in_w: u32, in_h: u32) -> Result<()> {
        if let Some(p) = &self.processor {
            if p.in_w == in_w && p.in_h == in_h {
                return Ok(());
            }
        }
        self.processor = None;
        let rate = DXGI_RATIONAL { Numerator: 60, Denominator: 1 };
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: in_w,
            InputHeight: in_h,
            OutputFrameRate: rate,
            OutputWidth: self.out_w,
            OutputHeight: self.out_h,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        unsafe {
            let enumerator = self.video_device.CreateVideoProcessorEnumerator(&content)?;
            let input_support = enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_B8G8R8A8_UNORM)?;
            let output_support = enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_NV12)?;
            if input_support & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT.0 as u32 == 0
                || output_support & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT.0 as u32 == 0
            {
                bail!(tr::tr!("la carte graphique ne sait pas convertir BGRA vers NV12"));
            }
            let processor = self.video_device.CreateVideoProcessor(&enumerator, 0)?;
            let input = create_texture(
                &self.device,
                in_w,
                in_h,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                D3D11_USAGE_DEFAULT,
                (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                0,
            )?;
            let input_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 },
                },
            };
            let mut input_view = None;
            self.video_device.CreateVideoProcessorInputView(&input, &enumerator, &input_desc, Some(&mut input_view))?;
            let output_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
            };
            let mut output_view = None;
            self.video_device.CreateVideoProcessorOutputView(
                &self.output,
                &enumerator,
                &output_desc,
                Some(&mut output_view),
            )?;

            let vc = &self.video_context;
            vc.VideoProcessorSetStreamFrameFormat(&processor, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            vc.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            vc.VideoProcessorSetStreamColorSpace(
                &processor,
                0,
                &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: INPUT_COLOR_SPACE },
            );
            vc.VideoProcessorSetOutputColorSpace(
                &processor,
                &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: OUTPUT_COLOR_SPACE },
            );
            let full = RECT { left: 0, top: 0, right: self.out_w as i32, bottom: self.out_h as i32 };
            vc.VideoProcessorSetOutputTargetRect(&processor, true, Some(&full));
            let black = D3D11_VIDEO_COLOR {
                Anonymous: D3D11_VIDEO_COLOR_0 { RGBA: D3D11_VIDEO_COLOR_RGBA { R: 0.0, G: 0.0, B: 0.0, A: 1.0 } },
            };
            vc.VideoProcessorSetOutputBackgroundColor(&processor, false, &black);

            self.processor = Some(Processor {
                in_w,
                in_h,
                processor,
                input,
                input_view: input_view.context(tr::tr!("vue d'entrée du processeur vidéo"))?,
                output_view: output_view.context(tr::tr!("vue de sortie du processeur vidéo"))?,
                _enumerator: enumerator,
            });
        }
        Ok(())
    }
}

impl VideoCapture for WgcCapture {
    fn poll(&mut self) -> Result<Option<Nv12Frame>> {
        // Ne garder que l'image la plus récente.
        let mut latest: Option<Direct3D11CaptureFrame> = None;
        loop {
            match self.pool.TryGetNextFrame() {
                Ok(frame) => {
                    if let Some(previous) = latest.replace(frame) {
                        let _ = previous.Close();
                    }
                }
                // Pointeur nul : aucune nouvelle image.
                Err(e) if e.code().is_ok() => break,
                Err(e) => return Err(e).context(tr::tr!("lecture d'une image capturée")),
            }
        }
        let Some(frame) = latest else {
            return Ok(None);
        };
        let content = frame.ContentSize()?;
        let result = self.process(&frame);
        let _ = frame.Close();

        // Fenêtre redimensionnée : les images suivantes auront la bonne taille.
        if content.Width > 0
            && content.Height > 0
            && (content.Width != self.pool_size.Width || content.Height != self.pool_size.Height)
        {
            self.pool.Recreate(&self.winrt_device, PIXEL_FORMAT, POOL_BUFFERS, content)?;
            self.pool_size = content;
        }
        result.map(Some)
    }
}

impl Drop for WgcCapture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// Demande, une seule fois, l'autorisation de capturer sans bordure jaune.
/// Sans réponse rapide, on continue : la bordure restera simplement visible.
fn request_borderless_access() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let operation = match GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless) {
            Ok(operation) => operation,
            Err(e) => {
                log::debug!("capture sans bordure indisponible : {e}");
                return;
            }
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        // État 0 : opération encore en cours.
        while operation.Status().map(|s| s.0 == 0).unwrap_or(false) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        match operation.GetResults() {
            Ok(status) => log::info!("capture sans bordure : autorisation {}", status.0),
            Err(e) => log::debug!("capture sans bordure : pas de réponse ({e})"),
        }
    });
}

fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .context(tr::tr!("création du périphérique Direct3D 11"))?;
    }
    Ok((
        device.context(tr::tr!("périphérique Direct3D absent"))?,
        context.context(tr::tr!("contexte Direct3D absent"))?,
    ))
}

fn create_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    usage: D3D11_USAGE,
    bind_flags: u32,
    cpu_access: u32,
) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: usage,
        BindFlags: bind_flags,
        CPUAccessFlags: cpu_access,
        MiscFlags: 0,
    };
    let mut texture = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture))? };
    texture.context(tr::tr!("texture Direct3D absente"))
}

/// Recopie une image NV12 relue depuis la carte graphique en un tampon
/// compact (plan Y puis plan UV entrelacé), sans les marges de ligne.
fn copy_nv12(mapped: &D3D11_MAPPED_SUBRESOURCE, width: usize, height: usize) -> Vec<u8> {
    let pitch = mapped.RowPitch as usize;
    let base = mapped.pData as *const u8;
    let mut out = Vec::with_capacity(width * height * 3 / 2);
    unsafe {
        for row in 0..height {
            out.extend_from_slice(std::slice::from_raw_parts(base.add(row * pitch), width));
        }
        let chroma = base.add(pitch * height);
        for row in 0..height / 2 {
            out.extend_from_slice(std::slice::from_raw_parts(chroma.add(row * pitch), width));
        }
    }
    out
}
