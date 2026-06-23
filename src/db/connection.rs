use rusqlite::{Connection, Result};
use std::path::Path;

pub fn init<P: AsRef<Path>>(path: P) -> Result<Connection> {
    crate::db::schema::initialize_db(path)
}
