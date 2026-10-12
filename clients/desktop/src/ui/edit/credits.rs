//! Credits (a track's or an album's): each artist with its role, the name it's credited
//! under ("credited as": its own, or one of its aliases), in order. The artist text people
//! see is made from these by the server, never typed. New artists (rows without an id) are
//! made when the album is saved.

pub use acetate_core::editor::forms::CreditRow;
pub use acetate_core::editor::forms::{make_new_artists, rows_from, take_ids};
