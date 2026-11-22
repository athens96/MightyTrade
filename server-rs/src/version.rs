use std::fs;
use std::path::Path;

pub fn read_version() -> String {
    for candidate in ["../VERSION", "VERSION", "/opt/mighty-trade/VERSION"] {
        if let Ok(raw) = fs::read_to_string(Path::new(candidate)) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    env!("CARGO_PKG_VERSION").to_string()
}
