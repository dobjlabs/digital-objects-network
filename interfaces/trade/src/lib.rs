//! Two-party swap demo over the joint-transaction machinery.
//!
//! One negotiation exchange completes the plan, then two exchanges carry
//! three proving sessions. The initiator assembles, finalizes, and posts.

pub mod engine;
pub mod local;
pub mod net;
pub mod post;
pub mod protocol;
pub mod ui;

#[cfg(test)]
mod tests;
