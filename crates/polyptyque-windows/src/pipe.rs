//! Tubes nommés par lesquels l'image et le son bruts sont envoyés à FFmpeg.

use std::fs::File;
use std::io::Write;
use std::os::windows::io::{FromRawHandle, RawHandle};

use anyhow::{bail, Result};
use polyptyque_core::platform::StreamPipe;
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, HANDLE};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_OUTBOUND};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};

/// Taille du tampon côté système : quelques images 1080p.
const OUT_BUFFER: u32 = 16 * 1024 * 1024;

pub struct NamedPipe {
    path: String,
    handle: HANDLE,
}

// La poignée n'est utilisée que par un seul fil à la fois.
unsafe impl Send for NamedPipe {}

impl NamedPipe {
    pub fn create(name: &str) -> Result<Self> {
        let path = format!(r"\\.\pipe\{name}");
        let handle = unsafe {
            CreateNamedPipeW(
                &HSTRING::from(path.as_str()),
                PIPE_ACCESS_OUTBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                OUT_BUFFER,
                0,
                0,
                None,
            )
        };
        if handle.is_invalid() {
            bail!(tr::tr!("création du tube {} impossible : {}", path, std::io::Error::last_os_error()));
        }
        Ok(Self { path, handle })
    }
}

impl StreamPipe for NamedPipe {
    fn ffmpeg_path(&self) -> String {
        self.path.clone()
    }

    fn connect(self: Box<Self>) -> Result<Box<dyn Write + Send>> {
        let handle = self.handle;
        let path = self.path.clone();
        // La poignée change de propriétaire : elle sera fermée par le `File`.
        std::mem::forget(self);
        if let Err(e) = unsafe { ConnectNamedPipe(handle, None) } {
            if e.code() != ERROR_PIPE_CONNECTED.to_hresult() {
                unsafe {
                    let _ = CloseHandle(handle);
                }
                bail!(tr::tr!("connexion de FFmpeg au tube {} : {}", path, e));
            }
        }
        let file = unsafe { File::from_raw_handle(handle.0 as RawHandle) };
        Ok(Box::new(file))
    }
}

impl Drop for NamedPipe {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}
