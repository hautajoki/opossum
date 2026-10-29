//! Place versions.

use crate::{Call, Client, Error, ErrorKind, SAVE_PLACE, id};

/// Roblox's limit on a place file.
pub const MAX_PLACE: usize = 100 << 20;

impl Client {
    /// Saves `rbxl` as a new version of the place, `Published` (live) or `Saved` (for exact-version execution), and
    /// returns its number. Never retried once sent: an ambiguous failure at worst leaves Roblox an extra version, which
    /// is harmless. A 409 means the place is open in Studio.
    pub fn save_place(&self, universe: u64, place: u64, rbxl: &[u8], published: bool) -> Result<u64, Error> {
        let kind = if published { "Published" } else { "Saved" };
        let path = format!("universes/v1/{universe}/places/{place}/versions?versionType={kind}");
        let call = Call {
            op: SAVE_PLACE,
            method: "POST",
            path,
            body: Some(("application/octet-stream", rbxl)),
            create: true,
            long: true,
        };
        if universe == 0 || place == 0 || rbxl.is_empty() || rbxl.len() > MAX_PLACE {
            let message = format!(
                "universe {universe} and place {place} must be positive and the file 1 byte to 100 MiB ({} bytes)",
                rbxl.len()
            );
            return Err(self.error(&call, ErrorKind::Local, message));
        }
        let v = self.json(&call)?;
        id(&v["versionNumber"])
            .ok_or_else(|| self.error(&call, ErrorKind::Decode, format!("no positive versionNumber in {v}")))
    }
}

#[cfg(test)]
mod tests {
    use crate::mock::{Mock, Reply};
    use crate::{Client, Clock, ErrorKind};

    #[test]
    fn a_save_sends_the_bytes_and_reads_a_text_plain_version() {
        let mock = Mock::start();
        let body = br#"{"versionNumber":17}"#.to_vec();
        let headers = vec![("Content-Type".into(), "text/plain".into())];
        mock.on("POST", "universes/v1/12/places/34/versions", Reply::Status { status: 200, headers, body });
        let c = Client::new("test-key", &mock.base, Clock::Real).unwrap();
        assert_eq!(c.save_place(12, 34, b"<roblox!rbxl", false).unwrap(), 17);
        let r = &mock.requests()[0];
        assert_eq!(r.query, "versionType=Saved");
        assert_eq!(r.body, b"<roblox!rbxl");
        assert_eq!(r.header("content-type"), Some("application/octet-stream"));
        assert_eq!(r.header("content-length"), Some("12"));
        assert_eq!(r.header("x-api-key"), Some("test-key"));
    }

    #[test]
    fn a_busy_place_says_studio_holds_it_and_is_not_retried() {
        let mock = Mock::start();
        let busy = Reply::Status { status: 409, headers: vec![], body: b"Server is busy".to_vec() };
        mock.on("POST", "universes/v1/12/places/34/versions", busy);
        let c = Client::new("test-key", &mock.base, Clock::Real).unwrap();
        let err = c.save_place(12, 34, b"x", true).unwrap_err();
        assert_eq!(err.diag(), "cloud.studioOpen");
        assert!(err.to_string().contains("Server is busy; the place is open in Studio"), "{err}");
        assert_eq!(mock.requests()[0].query, "versionType=Published");

        let too_big = vec![0; (100 << 20) + 1];
        for (u, p, bytes) in [(0, 34, &b"x"[..]), (12, 0, b"x"), (12, 34, b""), (12, 34, &too_big)] {
            assert_eq!(c.save_place(u, p, bytes, false).unwrap_err().kind, ErrorKind::Local);
        }
        assert_eq!(mock.requests().len(), 1);
    }
}
