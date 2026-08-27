//! Backend game logic for the terminal Pong game.
//!
//! Pure simulation: state, physics, and rules only. This crate performs no
//! I/O and knows nothing about terminals. The frontend crate (`pong-tui`)
//! communicates with it exclusively through the message protocol described
//! in ARCHITECTURE.md.
