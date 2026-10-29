//! A universe's resources: game passes, developer products, badges, the experience's name, description and icon, and
//! its thumbnails. A create (a pass, a product, a badge, a thumbnail upload) is not idempotent: the caller saves its
//! intent first, and it is never resent once a byte may have reached Roblox. Every other call sets a value and is
//! retried like a GET.

use serde_json::{Map, Value, json};

use crate::{Call, Client, Creator, Error, ErrorKind, Part, id, multipart};

/// Roblox shows at most this many thumbnails per language.
pub const MAX_THUMBNAILS: usize = 10;
/// Thumbnails are managed for one language.
const LANGUAGE: &str = "en_us";

/// An icon or a thumbnail as it is sent: PNG or JPEG bytes.
#[derive(Clone, Copy)]
pub struct Image<'a> {
    pub mime: &'static str,
    pub bytes: &'a [u8],
}

/// A pass's or a product's fields. A create sends every one given (and needs `name`); an update only the ones that
/// change. `None` leaves Roblox's value alone, and `Some("")` clears a description.
#[derive(Clone, Copy, Default)]
pub struct Sale<'a> {
    pub name: Option<&'a str>,
    pub description: Option<&'a str>,
    pub price: Option<u64>,
    pub for_sale: Option<bool>,
    pub managed_pricing: Option<bool>,
    pub icon: Option<Image<'a>>,
}

/// A badge to create. It costs `expected_cost` Robux, paid by `payer`, and Roblox refuses it when the fee is not
/// exactly that.
pub struct NewBadge<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub payer: Creator,
    pub expected_cost: u64,
    pub enabled: bool,
    pub icon: Image<'a>,
}

/// What an update of a badge changes; the icon is its own call.
#[derive(Clone, Copy, Default)]
pub struct BadgeChange<'a> {
    pub name: Option<&'a str>,
    pub description: Option<&'a str>,
    pub enabled: Option<bool>,
}

enum Commerce {
    Pass,
    Product,
}

impl Client {
    pub fn create_pass(&self, universe: u64, s: &Sale) -> Result<u64, Error> {
        self.create_sale(Commerce::Pass, universe, s)
    }

    pub fn update_pass(&self, universe: u64, id: u64, s: &Sale) -> Result<(), Error> {
        self.update_sale(Commerce::Pass, universe, id, s)
    }

    pub fn create_product(&self, universe: u64, s: &Sale) -> Result<u64, Error> {
        self.create_sale(Commerce::Product, universe, s)
    }

    pub fn update_product(&self, universe: u64, id: u64, s: &Sale) -> Result<(), Error> {
        self.update_sale(Commerce::Product, universe, id, s)
    }

    fn create_sale(&self, what: Commerce, universe: u64, s: &Sale) -> Result<u64, Error> {
        let (op, path, field) = match what {
            Commerce::Pass => {
                ("create game pass", format!("game-passes/v1/universes/{universe}/game-passes"), "gamePassId")
            }
            Commerce::Product => (
                "create developer product",
                format!("developer-products/v2/universes/{universe}/developer-products"),
                "productId",
            ),
        };
        let invalid = if s.name.is_none() { Some("a create needs a name".to_owned()) } else { sale_error(s) };
        let (mime, body) = sale_body(s);
        let call = Call { op, method: "POST", path, body: Some((&mime, &body)), create: true, long: true };
        self.check(&call, &[universe], invalid)?;
        let v = self.json(&call)?;
        self.created(&call, &v, field)
    }

    fn update_sale(&self, what: Commerce, universe: u64, id: u64, s: &Sale) -> Result<(), Error> {
        let (op, path) = match what {
            Commerce::Pass => ("update game pass", format!("game-passes/v1/universes/{universe}/game-passes/{id}")),
            Commerce::Product => (
                "update developer product",
                format!("developer-products/v2/universes/{universe}/developer-products/{id}"),
            ),
        };
        let empty = s.name.is_none()
            && s.description.is_none()
            && s.price.is_none()
            && s.for_sale.is_none()
            && s.managed_pricing.is_none()
            && s.icon.is_none();
        let invalid = if empty { Some("an update must change something".to_owned()) } else { sale_error(s) };
        let (mime, body) = sale_body(s);
        let call = Call { op, method: "PATCH", path, body: Some((&mime, &body)), create: false, long: true };
        self.check(&call, &[universe, id], invalid)?;
        self.send(&call).map(drop)
    }

    pub fn create_badge(&self, universe: u64, b: &NewBadge) -> Result<u64, Error> {
        let (payment, payer) = match b.payer {
            Creator::User(id) => ("1", id),
            Creator::Group(id) => ("2", id),
        };
        let cost = b.expected_cost.to_string();
        let enabled = b.enabled.to_string();
        let file = filename(b.icon);
        let parts = [
            text("name", b.name),
            text("description", b.description),
            text("paymentSourceType", payment),
            text("expectedCost", &cost),
            text("isActive", &enabled),
            Part { name: "files", filename: &file, mime: b.icon.mime, bytes: b.icon.bytes },
        ];
        let (mime, body) = multipart(&parts);
        let path = format!("legacy-badges/v1/universes/{universe}/badges");
        let call =
            Call { op: "create badge", method: "POST", path, body: Some((&mime, &body)), create: true, long: true };
        let invalid = if blank(b.name) || blank(b.description) {
            Some("a badge needs a name and a description".to_owned())
        } else {
            image_error(b.icon)
        };
        self.check(&call, &[universe, payer], invalid)?;
        let v = self.json(&call)?;
        self.created(&call, &v, "id")
    }

    pub fn update_badge(&self, id: u64, c: &BadgeChange) -> Result<(), Error> {
        let mut fields = Map::new();
        if let Some(name) = c.name {
            fields.insert("name".into(), name.into());
        }
        if let Some(description) = c.description {
            fields.insert("description".into(), description.into());
        }
        if let Some(enabled) = c.enabled {
            fields.insert("enabled".into(), enabled.into());
        }
        let invalid = if fields.is_empty() {
            Some("an update must change something".to_owned())
        } else if c.name.is_some_and(blank) || c.description.is_some_and(blank) {
            Some("a badge's name and description must not be blank".to_owned())
        } else {
            None
        };
        let body = Value::Object(fields).to_string();
        self.set("update badge", "PATCH", format!("legacy-badges/v1/badges/{id}"), &body, &[id], invalid)
    }

    /// Replaces a badge's icon.
    pub fn badge_icon(&self, id: u64, icon: Image) -> Result<(), Error> {
        let path = format!("legacy-publish/v1/badges/{id}/icon");
        self.set_image("update badge icon", path, "Files", icon, &[id])
    }

    /// Sets the experience's name and description, which Roblox takes from its root place; `None` leaves one alone.
    pub fn update_experience(
        &self,
        universe: u64,
        root: u64,
        name: Option<&str>,
        description: Option<&str>,
    ) -> Result<(), Error> {
        let mut body = json!({ "path": format!("universes/{universe}/places/{root}") });
        let mut mask = Vec::new();
        if let Some(name) = name {
            body["displayName"] = name.into();
            mask.push("displayName");
        }
        if let Some(description) = description {
            body["description"] = description.into();
            mask.push("description");
        }
        let invalid = if mask.is_empty() {
            Some("an update must change something".to_owned())
        } else if name.is_some_and(blank) {
            Some("the experience's name must not be blank".to_owned())
        } else {
            None
        };
        let path = format!("cloud/v2/universes/{universe}/places/{root}?updateMask={}", mask.join("%2C"));
        self.set("update experience", "PATCH", path, &body.to_string(), &[universe, root], invalid)
    }

    /// Replaces the experience's icon.
    pub fn experience_icon(&self, universe: u64, icon: Image) -> Result<(), Error> {
        let path = format!("legacy-game-internationalization/v1/game-icon/games/{universe}/language-codes/{LANGUAGE}");
        self.set_image("update experience icon", path, "request.files", icon, &[universe])
    }

    /// Adds a thumbnail to the end of the carousel and returns its media id. Not idempotent: each upload is another
    /// thumbnail.
    pub fn upload_thumbnail(&self, universe: u64, image: Image) -> Result<u64, Error> {
        let file = filename(image);
        let parts =
            [Part { name: "gameThumbnailRequest.files", filename: &file, mime: image.mime, bytes: image.bytes }];
        let (mime, body) = multipart(&parts);
        let call = Call {
            op: "upload thumbnail",
            method: "POST",
            path: format!("{}/image", thumbnails(universe)),
            body: Some((&mime, &body)),
            create: true,
            long: true,
        };
        self.check(&call, &[universe], image_error(image))?;
        let v = self.json(&call)?;
        self.created(&call, &v, "mediaAssetId")
    }

    /// Sets a thumbnail's alt text; `""` clears it.
    pub fn thumbnail_alt_text(&self, universe: u64, id: u64, alt: &str) -> Result<(), Error> {
        let body = json!({ "thumbnailId": id, "altText": alt }).to_string();
        let path = format!("{}/alt-text", thumbnails(universe));
        self.set("update thumbnail alt text", "POST", path, &body, &[universe, id], None)
    }

    /// Puts the carousel in this order.
    pub fn order_thumbnails(&self, universe: u64, ids: &[u64]) -> Result<(), Error> {
        let unique = ids.iter().enumerate().all(|(i, id)| !ids[..i].contains(id));
        let invalid = (ids.is_empty() || ids.len() > MAX_THUMBNAILS || !unique)
            .then(|| format!("the order must name 1 to {MAX_THUMBNAILS} different thumbnails, not {ids:?}"));
        let mut positive = ids.to_vec();
        positive.push(universe);
        let body = json!({ "mediaAssetIds": ids }).to_string();
        let path = format!("{}/images/order", thumbnails(universe));
        self.set("order thumbnails", "POST", path, &body, &positive, invalid)
    }

    /// Deletes a thumbnail. One already gone counts as deleted, so a cleanup can always be run again.
    pub fn delete_thumbnail(&self, universe: u64, id: u64) -> Result<(), Error> {
        let call = Call {
            op: "delete thumbnail",
            method: "DELETE",
            path: format!("{}/images/{id}", thumbnails(universe)),
            body: None,
            create: false,
            long: false,
        };
        self.check(&call, &[universe, id], None)?;
        match self.send(&call) {
            Err(e) if e.kind == ErrorKind::Status(404) => Ok(()),
            sent => sent.map(drop),
        }
    }

    /// A JSON request that sets a value.
    fn set(
        &self,
        op: &'static str,
        method: &'static str,
        path: String,
        body: &str,
        ids: &[u64],
        invalid: Option<String>,
    ) -> Result<(), Error> {
        let call =
            Call { op, method, path, body: Some(("application/json", body.as_bytes())), create: false, long: false };
        self.check(&call, ids, invalid)?;
        self.send(&call).map(drop)
    }

    /// A request that replaces an image in place.
    fn set_image(&self, op: &'static str, path: String, part: &str, image: Image, ids: &[u64]) -> Result<(), Error> {
        let file = filename(image);
        let (mime, body) = multipart(&[Part { name: part, filename: &file, mime: image.mime, bytes: image.bytes }]);
        let call = Call { op, method: "POST", path, body: Some((&mime, &body)), create: false, long: true };
        self.check(&call, ids, image_error(image))?;
        self.send(&call).map(drop)
    }

    /// Refuses before sending anything: an id that is not positive, or what `invalid` says.
    fn check(&self, call: &Call, ids: &[u64], invalid: Option<String>) -> Result<(), Error> {
        let invalid = if ids.contains(&0) { Some(format!("ids must be positive, not {ids:?}")) } else { invalid };
        match invalid {
            Some(message) => Err(self.error(call, ErrorKind::Local, message)),
            None => Ok(()),
        }
    }

    /// The id a create answered with in `field`.
    fn created(&self, call: &Call, v: &Value, field: &str) -> Result<u64, Error> {
        id(&v[field]).ok_or_else(|| self.error(call, ErrorKind::Decode, format!("no positive {field} in {v}")))
    }
}

fn thumbnails(universe: u64) -> String {
    format!("legacy-game-internationalization/v1/game-thumbnails/games/{universe}/language-codes/{LANGUAGE}")
}

fn blank(s: &str) -> bool {
    s.trim().is_empty()
}

fn text<'a>(name: &'a str, value: &'a str) -> Part<'a> {
    Part { name, filename: "", mime: "", bytes: value.as_bytes() }
}

/// The file part's name: ours, by the image's type, so it needs no quoting and its extension matches its MIME.
fn filename(image: Image) -> String {
    format!("image.{}", if image.mime == "image/jpeg" { "jpg" } else { "png" })
}

fn image_error(image: Image) -> Option<String> {
    if image.bytes.is_empty() || !matches!(image.mime, "image/png" | "image/jpeg") {
        return Some(format!("an image must be PNG or JPEG bytes ({}, {} bytes)", image.mime, image.bytes.len()));
    }
    None
}

fn sale_error(s: &Sale) -> Option<String> {
    if s.name.is_some_and(blank) {
        return Some("the name must not be blank".into());
    }
    if let Some(price) = s.price.filter(|p| !(1..=1_000_000_000).contains(p)) {
        return Some(format!("the price {price} must be 1 to 1000000000 Robux"));
    }
    s.icon.and_then(image_error)
}

/// A pass's or a product's form: the fields given, then the icon.
fn sale_body(s: &Sale) -> (String, Vec<u8>) {
    let (price, for_sale, managed) =
        (s.price.map(|p| p.to_string()), s.for_sale.map(|b| b.to_string()), s.managed_pricing.map(|b| b.to_string()));
    let fields = [
        ("name", s.name),
        ("description", s.description),
        ("isForSale", for_sale.as_deref()),
        ("price", price.as_deref()),
        ("isManagedPricingEnabled", managed.as_deref()),
    ];
    let mut parts: Vec<Part> = fields.iter().filter_map(|&(name, value)| Some(text(name, value?))).collect();
    let file = s.icon.map(filename).unwrap_or_default();
    if let Some(icon) = s.icon {
        parts.push(Part { name: "imageFile", filename: &file, mime: icon.mime, bytes: icon.bytes });
    }
    multipart(&parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Clock;
    use crate::mock::{Mock, Reply};

    const PNG: Image = Image { mime: "image/png", bytes: b"\x89PNG" };

    #[test]
    fn a_product_create_sends_its_declared_fields_and_an_explicit_empty_description() {
        let mock = Mock::start();
        mock.on("POST", "developer-products/v2/universes/7/", Reply::json(200, &json!({"productId": "41"})));
        let c = Client::new("test-key", &mock.base, Clock::Real).unwrap();
        let sale =
            Sale { name: Some("Coins"), description: Some(""), price: Some(25), icon: Some(PNG), ..Sale::default() };
        assert_eq!(c.create_product(7, &sale).unwrap(), 41);
        let r = &mock.requests()[0];
        let body = String::from_utf8_lossy(&r.body);
        let part = "form-data; name=\"";
        let names: Vec<&str> =
            body.match_indices(part).map(|(at, _)| body[at + part.len()..].split('"').next().unwrap()).collect();
        assert_eq!(names, ["name", "description", "price", "imageFile"]);
        assert!(body.contains("name=\"description\"\r\n\r\n\r\n"), "{body}");
        assert!(body.contains("name=\"imageFile\"; filename=\"image.png\"\r\nContent-Type: image/png\r\n"), "{body}");
        assert_eq!(r.header("content-length"), Some(r.body.len().to_string().as_str()));
    }

    #[test]
    fn invalid_requests_are_refused_before_anything_is_sent() {
        let mock = Mock::start();
        let c = Client::new("test-key", &mock.base, Clock::Real).unwrap();
        let named = Sale { name: Some("VIP"), ..Sale::default() };
        let errors = [
            c.create_pass(7, &Sale { price: Some(10), ..Sale::default() }).map(drop),
            c.create_pass(7, &Sale { price: Some(0), ..named }).map(drop),
            c.create_pass(0, &named).map(drop),
            c.update_pass(7, 3, &Sale::default()),
            c.update_product(
                7,
                3,
                &Sale { icon: Some(Image { mime: "image/gif", bytes: b"GIF8" }), ..Sale::default() },
            ),
            c.update_badge(3, &BadgeChange { description: Some(" "), ..BadgeChange::default() }),
            c.update_experience(7, 8, None, None),
            c.upload_thumbnail(7, Image { mime: "image/png", bytes: b"" }).map(drop),
            c.order_thumbnails(7, &[1, 2, 1]),
            c.order_thumbnails(7, &[]),
            c.delete_thumbnail(7, 0),
        ];
        for e in errors {
            assert_eq!(e.unwrap_err().kind, ErrorKind::Local);
        }
        assert!(mock.requests().is_empty());
    }

    #[test]
    fn a_thumbnail_already_gone_is_deleted_and_an_update_is_retried_where_a_create_is_not() {
        let mock = Mock::start();
        mock.on("DELETE", "legacy-game-internationalization/", Reply::json(404, &json!({})));
        mock.on("PATCH", "cloud/v2/universes/7/places/8", Reply::json(503, &json!({})));
        mock.on("PATCH", "cloud/v2/universes/7/places/8", Reply::json(200, &json!({})));
        mock.on("POST", "legacy-game-internationalization/", Reply::json(503, &json!({})));
        let c = Client::new("test-key", &mock.base, Clock::Fake(crate::Fake::new())).unwrap();
        c.delete_thumbnail(7, 55).unwrap();
        c.update_experience(7, 8, None, Some("")).unwrap();
        let err = c.upload_thumbnail(7, PNG).unwrap_err();
        assert!(!err.definite());
        let requests = mock.requests();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[1].query, "updateMask=description");
        assert_eq!(requests[1].json(), json!({"path": "universes/7/places/8", "description": ""}));
    }
}
