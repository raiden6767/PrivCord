//! Windows: voice notes without a lasting microphone permission.
//!
//! Chromium (and so WebView2) shows a peer connection's local network address as a random `.local` name, so a contact
//! never learns it. It shows the real address instead when the page holds a *lasting* microphone permission. Voice notes
//! need the microphone, and WebView2 keeps the person's answer in the profile by default, so after the first voice note
//! every connection offer carried the local address (METADATA_AUDIT.md). This answers the microphone request itself and
//! asks WebView2 not to save the answer, so the name stays hidden. It also clears an answer an earlier version saved.
//!
//! The request only comes from the Record button in the app's own page (nothing else is loaded: `frame-src 'none'`), and
//! Windows' own microphone privacy switch still applies.
//!
//! Not tried on a real Windows machine when it was written (it type-checks against the Windows target). To check by hand:
//! record a voice note, connect to a friend, and confirm the connection offer has `.local` names, not a 192.168.x.x address.

use tauri::WebviewWindow;
use webview2_com::{
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2PermissionRequestedEventArgs3, ICoreWebView2Profile4, ICoreWebView2_13,
        COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
        COREWEBVIEW2_PERMISSION_STATE_ALLOW, COREWEBVIEW2_PERMISSION_STATE_DEFAULT,
    },
    PermissionRequestedEventHandler, SetPermissionStateCompletedHandler,
};
use windows_core::{w, Interface};

/// Where the app's page is served from (Tauri's scheme on Windows, with or without https).
const ORIGINS: [windows_core::PCWSTR; 2] =
    [w!("http://tauri.localhost"), w!("https://tauri.localhost")];

pub fn install(window: &WebviewWindow) {
    let _ = window.with_webview(|webview| unsafe {
        let Ok(core) = webview.controller().CoreWebView2() else {
            return;
        };
        // Forget an answer an earlier version saved (nothing happens if the runtime is too old for this).
        if let Ok(profile) = core
            .cast::<ICoreWebView2_13>()
            .and_then(|c| c.Profile())
            .and_then(|p| p.cast::<ICoreWebView2Profile4>())
        {
            for origin in ORIGINS {
                let _ = profile.SetPermissionState(
                    COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
                    origin,
                    COREWEBVIEW2_PERMISSION_STATE_DEFAULT,
                    &SetPermissionStateCompletedHandler::create(Box::new(|_| Ok(()))),
                );
            }
        }
        let handler = PermissionRequestedEventHandler::create(Box::new(|_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
            args.PermissionKind(&mut kind)?;
            if kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE {
                args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
                // Answer this request only: do not keep the answer in the profile.
                if let Ok(args3) = args.cast::<ICoreWebView2PermissionRequestedEventArgs3>() {
                    let _ = args3.SetSavesInProfile(false);
                }
            }
            Ok(())
        }));
        let mut token = Default::default();
        let _ = core.add_PermissionRequested(&handler, &mut token);
    });
}
