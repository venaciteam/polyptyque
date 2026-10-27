//! Implémentation Windows de [`Platform`] :
//! - image : Windows.Graphics.Capture, conversion NV12 par la carte graphique ;
//! - son : capture WASAPI « process loopback » (un programme et ses
//!   sous-programmes), et micro ;
//! - fenêtres, processus, navigateurs installés, tubes nommés.

#![cfg(windows)]

mod audio;
mod browsers;
pub mod control_pipe;
pub mod hotkeys;
mod pipe;
mod process;
pub mod session_end;
pub mod single_instance;
pub mod startup;
mod system;
mod system_info;
pub mod tray;
mod video;
mod windows_list;

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

use polyptyque_core::platform::{
    AudioCapture, AudioDevice, BrowserInstall, Platform, StreamPipe, VideoCapture, WindowHandle, WindowInfo,
};

/// Tant qu'une programmation est active (créneau en cours ou à venir) :
/// mise en veille automatique et extinction de l'écran empêchées,
/// indépendamment de l'enregistrement. À appeler depuis n'importe quel fil.
pub fn keep_awake_for_schedules(on: bool) {
    system::keep_awake_for_schedules(on);
}

/// Langue des messages propres à Windows (voir [`polyptyque_core::i18n`]).
pub fn set_catalog(catalog: Option<std::sync::Arc<polyptyque_core::i18n::Catalog>>) {
    match catalog {
        Some(catalog) => tr::set_translator!(catalog),
        None => tr::set_translator!(polyptyque_core::i18n::French),
    }
}

/// Pas de fenêtre de console pour FFmpeg.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// FFmpeg ne reçoit pas le Ctrl+C de la console : c'est Polyptyque qui
/// l'arrête proprement en fermant ses entrées.
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

pub struct WindowsPlatform {
    own_pid: u32,
}

impl WindowsPlatform {
    pub fn new() -> Self {
        Self { own_pid: std::process::id() }
    }
}

impl Default for WindowsPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform for WindowsPlatform {
    fn list_windows(&self) -> Vec<WindowInfo> {
        windows_list::list(self.own_pid)
    }

    fn is_window_alive(&self, handle: WindowHandle) -> bool {
        windows_list::is_alive(handle)
    }

    fn is_window_minimized(&self, handle: WindowHandle) -> bool {
        windows_list::is_minimized(handle)
    }

    fn restore_window(&self, handle: WindowHandle) -> anyhow::Result<()> {
        windows_list::restore(handle)
    }

    fn close_window(&self, handle: WindowHandle) -> anyhow::Result<()> {
        windows_list::close(handle)
    }

    fn process_command_line(&self, pid: u32) -> Option<String> {
        process::command_line(pid)
    }

    fn detect_browsers(&self) -> Vec<BrowserInstall> {
        browsers::detect()
    }

    fn now_hns(&self) -> u64 {
        system::now_hns()
    }

    fn open_video_capture(
        &self,
        handle: WindowHandle,
        width: u32,
        height: u32,
        capture_cursor: bool,
    ) -> anyhow::Result<Box<dyn VideoCapture>> {
        Ok(Box::new(video::WgcCapture::open(handle, width, height, capture_cursor)?))
    }

    fn open_audio_capture(&self, pid: u32, sample_rate: u32, channels: u16) -> anyhow::Result<Box<dyn AudioCapture>> {
        Ok(Box::new(audio::WasapiCapture::process(pid, sample_rate, channels)?))
    }

    fn list_microphones(&self) -> Vec<AudioDevice> {
        audio::microphones()
    }

    fn open_microphone_capture(
        &self,
        device_id: Option<&str>,
        sample_rate: u32,
        channels: u16,
    ) -> anyhow::Result<Box<dyn AudioCapture>> {
        Ok(Box::new(audio::WasapiCapture::microphone(device_id, sample_rate, channels)?))
    }

    fn create_stream_pipe(&self, name: &str) -> anyhow::Result<Box<dyn StreamPipe>> {
        Ok(Box::new(pipe::NamedPipe::create(name)?))
    }

    fn prepare_encoder_process(&self, command: &mut Command) {
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }

    fn begin_recording_session(&self) {
        system::begin_session();
    }

    fn end_recording_session(&self) {
        system::end_session();
    }

    fn free_disk_space(&self, path: &Path) -> Option<u64> {
        system::free_space(path)
    }

    fn os_description(&self) -> String {
        system_info::os_description()
    }

    fn graphics_adapters(&self) -> Vec<String> {
        system_info::graphics_adapters()
    }

    fn graphics_driver_events(&self) -> Option<Vec<String>> {
        system_info::graphics_driver_events()
    }

    fn uses_twelve_hour_clock(&self) -> bool {
        system_info::uses_twelve_hour_clock()
    }

    fn shell_open(&self, target: &str) -> anyhow::Result<()> {
        system_info::shell_open(target)
    }

    fn reveal_file(&self, path: &Path) -> anyhow::Result<()> {
        system_info::reveal_file(path)
    }
}
