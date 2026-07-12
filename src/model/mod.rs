//! Wire-shape structs and their normalized output counterparts (D2), one
//! module per noun. Wire types deserialize leniently in the binary and deny
//! unknown fields under test — the same drift tripwire as the envelope.

pub mod action;
pub mod profile;
