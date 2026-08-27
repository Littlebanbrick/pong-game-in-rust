//! Backend runtime: owns the authoritative game thread.
//!
//! [`Backend::spawn`] starts a dedicated thread that advances a [`Game`] at
//! a fixed 60 ticks per second, bridges between the frontend's channels and
//! the simulation, and broadcasts a [`GameSnapshot`] after every tick. The
//! frontend never touches the `Game` directly — it only talks to these
//! channels. See ARCHITECTURE.md.
//!
//! The runtime is I/O-free (channels are not I/O), so it stays in
//! `pong-core` and remains testable.

use std::sync::mpsc::{Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::game::{DT, Game};
use crate::protocol::{GameSnapshot, InputEvent};

/// Error returned when the backend thread is gone but a snapshot is needed.
#[derive(Debug)]
pub struct BackendClosed;

impl std::fmt::Display for BackendClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "backend thread is not running")
    }
}

impl std::error::Error for BackendClosed {}

/// Running backend: a handle to its thread plus both channel ends.
///
/// * `input` — frontend sends [`InputEvent`]s here.
/// * `snapshots` — frontend receives one [`GameSnapshot`] per tick.
pub struct Backend {
    input: Sender<InputEvent>,
    snapshots: Receiver<GameSnapshot>,
    handle: Option<JoinHandle<()>>,
}

impl Backend {
    /// Spawns the authoritative simulation thread.
    pub fn spawn() -> Self {
        let (input_tx, input_rx) = std::sync::mpsc::channel();
        let (snapshot_tx, snapshot_rx) = std::sync::mpsc::channel();
        let handle = thread::spawn(|| run_backend(input_rx, snapshot_tx));
        Backend {
            input: input_tx,
            snapshots: snapshot_rx,
            handle: Some(handle),
        }
    }

    /// Sends an input event to the simulation thread.
    ///
    /// Failing to send can only mean the backend thread has exited, which
    /// the frontend treats as a shutdown; the error is swallowed on purpose.
    pub fn send(&self, event: InputEvent) {
        let _ = self.input.send(event);
    }

    /// Blocks until the next per-tick snapshot arrives.
    pub fn next_snapshot(&self) -> Result<GameSnapshot, BackendClosed> {
        self.snapshots.recv().map_err(|_| BackendClosed)
    }

    /// Tries to take the newest snapshot without blocking.
    ///
    /// Discards queued intermediate snapshots and returns the most recent
    /// one; `None` means nothing is queued yet.
    pub fn latest_snapshot(&self) -> Option<GameSnapshot> {
        let mut latest = self.snapshots.try_recv().ok()?;
        while let Ok(newer) = self.snapshots.try_recv() {
            latest = newer;
        }
        Some(latest)
    }

    /// Requests shutdown and waits for the thread to finish.
    pub fn join(mut self) {
        self.send(InputEvent::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.send(InputEvent::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Thread body: fixed-rate tick loop with input draining and snapshot
/// broadcasting.
fn run_backend(input: Receiver<InputEvent>, snapshots: Sender<GameSnapshot>) {
    let mut game = Game::new();
    let tick_duration = Duration::from_secs_f64(DT as f64);
    let mut next_tick = Instant::now();

    loop {
        // Drain all pending input; `Shutdown` exits immediately.
        while let Ok(event) = input.try_recv() {
            if matches!(event, InputEvent::Shutdown) {
                return;
            }
            game.handle_input(event);
        }

        game.tick();
        if snapshots.send(game.snapshot()).is_err() {
            return; // Frontend is gone; stop the thread.
        }

        // Sleep so that ticks start every `tick_duration`, absorbing drift
        // and catching up after a slow frame instead of lagging behind.
        next_tick += tick_duration;
        let now = Instant::now();
        if next_tick > now {
            thread::sleep(next_tick - now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Direction, GamePhase, Side};

    #[test]
    fn snapshots_stream_at_least_once_per_tick() {
        let backend = Backend::spawn();
        let first = backend.next_snapshot().expect("first snapshot");
        assert_eq!(first.phase, GamePhase::Playing);
        // Ball must be moving: two ticks must differ in position.
        let second = backend.next_snapshot().expect("second snapshot");
        assert_ne!(first.ball_x, second.ball_x);
        backend.join();
    }

    #[test]
    fn paddle_direction_flows_through_the_channel() {
        let backend = Backend::spawn();
        // Skip forward until we see the left paddle clearly off center.
        for _ in 0..600 {
            let before = backend.next_snapshot().expect("snapshot");
            backend.send(InputEvent::SetPaddleDirection {
                side: Side::Left,
                direction: Some(Direction::Up),
            });
            let after = backend.next_snapshot().expect("snapshot");
            if after.left_paddle_y < before.left_paddle_y {
                backend.join();
                return; // Input took effect: pass.
            }
        }
        panic!("left paddle never moved upward after 600 ticks");
    }

    #[test]
    fn shutdown_stops_the_thread() {
        let mut backend = Backend::spawn();
        let handle = backend.handle.take().expect("handle present");
        backend.send(InputEvent::Shutdown);
        thread::sleep(Duration::from_millis(50));
        assert!(handle.is_finished(), "thread should exit after Shutdown");
        // Snapshots stop arriving once the thread is gone.
        assert!(backend.next_snapshot().is_err());
    }

    #[test]
    fn latest_snapshot_drains_the_queue() {
        let backend = Backend::spawn();
        // Let some ticks queue up.
        thread::sleep(Duration::from_millis(100));
        let latest = backend.latest_snapshot().expect("queued snapshot");
        // After draining, the queue is empty (beyond at most one new tick).
        let drained = backend.latest_snapshot();
        if let Some(newer) = drained {
            assert!(newer.ball_x != latest.ball_x || newer.ball_y != latest.ball_y);
        }
        backend.join();
    }
}
