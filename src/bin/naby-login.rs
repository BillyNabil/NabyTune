//! NabyTune's sign-in window: Google's own sign-in page in WebView2, as YouTube Music's
//! website shows it. The listener types their password into Google's page, never into
//! NabyTune. Once Google lands back on music.youtube.com, the session's cookies are printed
//! on stdout (one `Cookie` header line) for NabyTune to keep, and the window closes.
//!
//! `naby-login --sign-out` forgets the session this window keeps between sign-ins.

#![windows_subsystem = "windows"]

use std::path::PathBuf;

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::{PageLoadEvent, WebContext, WebViewBuilder};

const SIGN_IN: &str = "https://accounts.google.com/ServiceLogin?ltmpl=music&service=youtube&passive=true&continue=https%3A%2F%2Fmusic.youtube.com%2F";

/// Where the sign-in window keeps its browser data (cookies), apart from everything else.
fn data_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("NabyTune").join("SignIn"))
}

enum Note {
    Loaded,
}

fn main() {
    if std::env::args().any(|a| a == "--sign-out") {
        if let Some(dir) = data_dir() {
            let _ = std::fs::remove_dir_all(dir);
        }
        return;
    }

    let event_loop = EventLoopBuilder::<Note>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title("Sign in to YouTube Music · NabyTune")
        .with_inner_size(LogicalSize::new(480.0, 720.0))
        .build(&event_loop)
        .expect("sign-in window");
    let mut context = WebContext::new(data_dir());
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(SIGN_IN)
        .with_on_page_load_handler(move |event, _| {
            if matches!(event, PageLoadEvent::Finished) {
                let _ = proxy.send_event(Note::Loaded);
            }
        })
        .build(&window)
        .expect("WebView2");

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            // Back on YouTube Music, signed in: its cookies carry a SAPISID.
            Event::UserEvent(Note::Loaded) if webview.url().is_ok_and(|u| u.starts_with("https://music.youtube.com")) => {
                let Ok(cookies) = webview.cookies_for_url("https://music.youtube.com") else { return };
                if cookies.iter().any(|c| matches!(c.name(), "SAPISID" | "__Secure-3PAPISID")) {
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
