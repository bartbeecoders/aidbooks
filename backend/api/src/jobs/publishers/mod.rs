//! Background publishers for distributing finished audiobooks. Each handler
//! takes a fully-narrated language version and ships it to a third-party.

pub mod animate;
pub mod youtube;
