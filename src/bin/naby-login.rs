//! NabyTune's sign-in window: Google's own sign-in page in WebView2, as YouTube Music's
//! website shows it. The listener types their password into Google's page, never into
//! NabyTune. Once Google lands back on music.youtube.com, the session's cookies are printed
//! on stdout (one `Cookie` header line) for NabyTune to keep, and the window closes.
//!
//! `naby-login --sign-out` forgets the session this window keeps between sign-ins.

#![windows_subsystem = "windows"]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::{WebContext, WebViewBuilder};

const SIGN_IN: &str = "https://accounts.google.com/ServiceLogin?ltmpl=music&service=youtube&continue=https%3A%2F%2Fmusic.youtube.com%2F";

/// Where the sign-in window keeps its browser data (cookies), apart from everything else.
fn data_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("NabyTune").join("SignIn"))
}

fn main() {
    if std::env::args().any(|a| a == "--sign-out") {
        if let Some(dir) = data_dir() {
            let _ = std::fs::remove_dir_all(dir);
        }
        return;
    }

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("Sign in to YouTube Music · NabyTune")
        .with_inner_size(LogicalSize::new(480.0, 720.0))
        .build(&event_loop);
    let Ok(window) = window else { std::process::exit(2) };
    let mut context = WebContext::new(data_dir().map(|dir| dir.with_file_name("SignInPrivate")));
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_incognito(true)
        .with_url(SIGN_IN)
        .build(&window);
    let Ok(webview) = webview else { std::process::exit(2) };

    let mut next_check = Instant::now();
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(next_check);
        match event {
            // Navigation can finish before the authenticated cookies are available.
            Event::MainEventsCleared if Instant::now() >= next_check => {
                next_check = Instant::now() + Duration::from_millis(750);
                *control_flow = ControlFlow::WaitUntil(next_check);
                if !webview.url().is_ok_and(|url| url == "https://music.youtube.com" || url.starts_with("https://music.youtube.com/")) {
                    return;
                }
                let Ok(cookies) = webview.cookies_for_url("https://music.youtube.com") else { return };
                if cookies.iter().any(|c| {
                    !c.value().is_empty() && matches!(c.name(), "SAPISID" | "__Secure-3PAPISID" | "__Secure-1PAPISID")
                }) {
                    let header = cookies.iter().map(|c| format!("{}={}", c.name(), c.value())).collect::<Vec<_>>().join("; ");
                    println!("{header}");
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => *control_flow = ControlFlow::Exit,
            _ => {}
        }
    });
}
