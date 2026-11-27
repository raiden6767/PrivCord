#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The desktop shell's relay sockets: one that looks open but has gone silent (what a laptop is left with after sleep or a
//! network change) is replaced; one that answers is kept.

use privcord_desktop_host::native_signaling::{NativeSignaling, Timing};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A WebSocket server that counts connections. `answer`: read what arrives (which answers pings); otherwise stay silent.
fn server(answer: bool) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(AtomicUsize::new(0));
    let count = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            count.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || {
                let Ok(mut ws) = tungstenite::accept(stream) else {
                    return;
                };
                if answer {
                    while ws.read().is_ok() {} // reading is what sends the pongs
                } else {
                    std::thread::sleep(Duration::from_secs(30)); // open, and says nothing
                }
            });
        }
    });
    (port, seen)
}

fn start(port: u16, timing: Timing) -> NativeSignaling {
    let s = NativeSignaling::start_with(Arc::new(|_, _| {}), Arc::new(|_| {}), timing);
    let cfg = format!(r#"{{"signaling":[{{"kind":"relay","url":"ws://127.0.0.1:{port}"}}]}}"#);
    s.configure(&cfg, None);
    s
}

fn wait_for(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < until {
        if ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{what}");
}

#[test]
fn a_silent_socket_is_replaced() {
    let (port, seen) = server(false);
    let _s = start(port, Timing::new(200, 1_200, 60_000));
    wait_for("it never connected", 5, || seen.load(Ordering::SeqCst) >= 1);
    wait_for("the silent socket was never replaced", 10, || {
        seen.load(Ordering::SeqCst) >= 2
    });
}

#[test]
fn a_socket_that_answers_is_kept() {
    let (port, seen) = server(true);
    let _s = start(port, Timing::new(200, 1_200, 60_000));
    wait_for("it never connected", 5, || seen.load(Ordering::SeqCst) >= 1);
    std::thread::sleep(Duration::from_secs(4)); // several times the dead-after time
    assert_eq!(
        seen.load(Ordering::SeqCst),
        1,
        "a healthy socket must not be replaced"
    );
}
