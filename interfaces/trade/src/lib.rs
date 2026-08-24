//! Two-party swap demo over the joint-transaction machinery.
//!
//! The protocol is the swap scenario's statement-graph schedule, run
//! between two processes instead of two `BuildContext`s in one test:
//! one data round completes the plan, then two exchanges carry the
//! three proving sessions. The initiator of a trade is its executor:
//! it proves the offer for the object it gives, receives the
//! counterparty's combined offer-plus-acceptance, assembles both legs,
//! finalizes, and posts.

pub mod engine;
pub mod local;
pub mod net;
pub mod post;
pub mod protocol;
pub mod ui;

#[cfg(test)]
mod tests;
