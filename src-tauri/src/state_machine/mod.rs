pub mod events;
pub mod reducer;

pub use events::Event;
pub use reducer::{reduce, Machine, Outcome};
