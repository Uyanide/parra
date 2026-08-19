//! The configuration file surface, one file per compositor backend.
//!
//! The fallback layer-shell namespace arrives as a parameter, so the program's name has
//! no second definition point here.

pub mod load;
pub mod merge;
pub mod schema;
pub mod watch;

pub use load::{ConfigError, Loaded, defaults, load};
pub use merge::{Config, Context, Invalid, resolve};
pub use schema::ConfigFile;
pub use watch::{WatchError, Watcher};
