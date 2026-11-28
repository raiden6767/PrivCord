#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::type_complexity
)]
//! The panel restarts the server with new choices. A bad choice must never leave the server down.

use privcord_server::{config::Config, protocols::Panel, supervise};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn free_tcp() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

type Log = Arc<Mutex<Vec<(bool, bool, bool)>>>; // (relay, nostr, turn) of each start

async fn until(what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "timed out waiting for: {what}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

struct Harness {
    log: Log,
    quit: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<Result<(), String>>,
    dir: tempfile::TempDir,
    restart: Arc<Mutex<Option<Box<dyn Fn() + Send>>>>,
}

fn run(mut cfg: Config, dir: tempfile::TempDir) -> Harness {
    cfg.server.data_dir = dir.path().into();
    cfg.relay.store_and_forward = false;
    let log: Log = Arc::default();
    let restart: Arc<Mutex<Option<Box<dyn Fn() + Send>>>> = Arc::default();
    let (quit, quit_rx) = tokio::sync::oneshot::channel::<()>();
    let (l, r) = (log.clone(), restart.clone());
    let task = tokio::spawn(supervise(
        cfg,
        async {
            let _ = quit_rx.await;
        },
        move |run| {
            let c = run.config();
            l.lock()
                .unwrap()
                .push((c.relay.enabled, c.nostr.enabled, c.turn.enabled));
            *r.lock().unwrap() = Some(Box::new(run.restart_handle()));
        },
    ));
    Harness {
        log,
        quit,
        task,
        dir,
        restart,
    }
}

impl Harness {
    fn ask_restart(&self) {
        (self.restart.lock().unwrap().as_ref().unwrap())();
    }
    async fn finish(self) {
        let _ = self.quit.send(());
        self.task.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn new_choices_are_applied_by_a_restart_on_the_same_port_even_with_the_disk_store() {
    let mut cfg = Config::default();
    cfg.server.listen = format!("127.0.0.1:{}", free_tcp());
    cfg.admin.enabled = false;
    cfg.nostr.enabled = false;
    cfg.nostr.backend = "disk".into();
    let h = run(cfg, tempfile::tempdir().unwrap());
    until("first start", || h.log.lock().unwrap().len() == 1).await;
    assert_eq!(h.log.lock().unwrap()[0], (true, false, false));

    Panel {
        nostr: Some(true),
        ..Panel::default()
    }
    .save(h.dir.path())
    .unwrap();
    h.ask_restart();
    until("second start", || h.log.lock().unwrap().len() == 2).await;
    assert_eq!(
        h.log.lock().unwrap()[1],
        (true, true, false),
        "the disk store opened again after the stop"
    );
    assert!(privcord_server::protocols::last_error(h.dir.path()).is_none());
    h.finish().await;
}

#[tokio::test]
async fn choices_that_cannot_start_put_the_server_back_to_what_worked_and_say_why() {
    // A UDP port that is taken: the server cannot answer STUN there.
    let held = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut cfg = Config::default();
    cfg.server.listen = format!("127.0.0.1:{}", free_tcp());
    cfg.admin.enabled = false;
    cfg.nostr.enabled = false;
    cfg.turn.listen = held.local_addr().unwrap().to_string();
    let h = run(cfg, tempfile::tempdir().unwrap());
    until("first start", || h.log.lock().unwrap().len() == 1).await;

    Panel {
        turn: Some("stun".into()),
        ..Panel::default()
    }
    .save(h.dir.path())
    .unwrap();
    h.ask_restart();
    until("the fallback start", || h.log.lock().unwrap().len() == 2).await;
    assert_eq!(
        h.log.lock().unwrap()[1],
        (true, false, false),
        "back to what worked"
    );
    let why = privcord_server::protocols::last_error(h.dir.path()).expect("the reason is kept");
    assert!(why.contains("went back"), "{why}");
    assert_eq!(
        Panel::load(h.dir.path()),
        Panel::default(),
        "the bad choice is not left in the file"
    );

    // and once the port is free, the same choice works
    drop(held);
    Panel {
        turn: Some("stun".into()),
        ..Panel::default()
    }
    .save(h.dir.path())
    .unwrap();
    h.ask_restart();
    until("the good start", || h.log.lock().unwrap().len() == 3).await;
    assert_eq!(h.log.lock().unwrap()[2], (true, false, true));
    assert!(privcord_server::protocols::last_error(h.dir.path()).is_none());
    h.finish().await;
}
