//! JSON property values (model.json `properties` and `attributes`, properties-file tables) resolved against the
//! reflection database into typed variants. The shorthand forms are Rojo's: `[x, y, z]` is whatever the
//! property's type says, an enum is its item name, and `{"Vector3": [...]}` names the type outright.

use rbx_dom_weak::types::{
    Attributes, CFrame, Color3, Color3uint8, Content, ContentId, Enum, MaterialColors, Matrix3,
    NumberRange, Rect, Tags, UDim, UDim2, Variant, VariantType, Vector2, Vector2int16, Vector3,
    Vector3int16,
};
use rbx_reflection::{DataType, PropertyDescriptor, PropertyKind, ReflectionDatabase};
use serde_json::Value;

pub fn db() -> &'static ReflectionDatabase<'static> {
    rbx_reflection_database::get().unwrap_or_else(|_| rbx_reflection_database::get_bundled())
}

pub fn is_class(name: &str) -> bool {
    db().classes.contains_key(name)
}

/// A property value: either a variant, or a Ref written as an instance path relative to its holder, resolved
/// once the whole tree exists.
pub enum Resolved {
    Value(Variant),
    Ref(String),
}

/// The canonical descriptor of `class.name`, looked up through superclasses and aliases.
pub fn descriptor(class: &str, name: &str) -> Option<&'static PropertyDescriptor<'static>> {
    let db = db();
    let mut class = db.classes.get(class)?;
    loop {
        if let Some(prop) = class.properties.get(name) {
            return match prop.kind {
                PropertyKind::Alias { alias_for } => descriptor(class.name, alias_for),
                _ => Some(prop),
            };
        }
        class = db.classes.get(class.superclass?)?;
    }
}

/// Resolves `class.name = json` to the canonical property name and its value.
pub fn property(class: &str, name: &str, json: &Value) -> Result<(&'static str, Resolved), String> {
    let Some(prop) = descriptor(class, name) else {
        return Err(format!("{class} has no property {name}"));
    };
    let resolved = match (&prop.data_type, json) {
        (DataType::Value(VariantType::Ref), Value::String(path)) => Resolved::Ref(path.clone()),
        (DataType::Enum(enum_name), Value::String(item)) => {
            let items = &db()
                .enums
                .get(enum_name)
                .ok_or(format!("unknown enum {enum_name}"))?
                .items;
            let value = items
                .get(item.as_str())
                .ok_or(format!("{item} is not an item of Enum.{enum_name}"))?;
            Resolved::Value(Variant::Enum(Enum::from_u32(*value)))
        }
        (DataType::Enum(_), Value::Number(n)) => {
            let value = n
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or(format!("{n} is not an enum value"))?;
            Resolved::Value(Variant::Enum(Enum::from_u32(value)))
        }
        (data_type, json) => Resolved::Value(value(data_type.ty(), json)?),
    };
    Ok((prop.name, resolved))
}

/// An attribute value: booleans, numbers (as Float64) and strings by shorthand, anything else fully qualified.
pub fn attribute(json: &Value) -> Result<Variant, String> {
    match json {
        Value::Bool(b) => Ok(Variant::Bool(*b)),
        Value::Number(n) => Ok(Variant::Float64(n.as_f64().unwrap_or_default())),
        Value::String(s) => Ok(Variant::String(s.clone())),
        Value::Object(_) => qualified(json),
        _ => Err(format!(
            "attribute value {json} needs its type, as {{\"Vector3\": [...]}}"
        )),
    }
}

pub fn attributes(json: &Value) -> Result<Attributes, String> {
    let Value::Object(map) = json else {
        return Err("attributes must be an object".into());
    };
    let mut attributes = Attributes::new();
    for (name, value) in map {
        attributes.insert(
            name.clone(),
            attribute(value).map_err(|e| format!("attribute {name}: {e}"))?,
        );
    }
    Ok(attributes)
}

fn qualified(json: &Value) -> Result<Variant, String> {
    serde_json::from_value::<Variant>(json.clone()).map_err(|e| format!("{json}: {e}"))
}

fn numbers<const N: usize>(json: &Value) -> Option<[f64; N]> {
    let Value::Array(items) = json else {
        return None;
    };
    if items.len() != N {
        return None;
    }
    let mut out = [0.0; N];
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = item.as_f64()?;
    }
    Some(out)
}

fn value(ty: VariantType, json: &Value) -> Result<Variant, String> {
    let v = match (ty, json) {
        (_, Value::Object(_))
            if !matches!(ty, VariantType::MaterialColors | VariantType::Attributes) =>
        {
            let v = qualified(json)?;
            if v.ty() != ty {
                return Err(format!("expected {ty:?}, got {:?}", v.ty()));
            }
            Some(v)
        }
        (VariantType::Bool, Value::Bool(b)) => Some(Variant::Bool(*b)),
        (VariantType::String, Value::String(s)) => Some(Variant::String(s.clone())),
        (VariantType::ContentId, Value::String(s)) => {
            Some(Variant::ContentId(ContentId::from(s.as_str())))
        }
        (VariantType::Content, Value::String(s)) => {
            Some(Variant::Content(Content::from_uri(s.as_str())))
        }
        (VariantType::Float32, Value::Number(n)) => n.as_f64().map(|n| Variant::Float32(n as f32)),
        (VariantType::Float64, Value::Number(n)) => n.as_f64().map(Variant::Float64),
        (VariantType::Int32, Value::Number(n)) => n
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .map(Variant::Int32),
        (VariantType::Int64, Value::Number(n)) => n.as_i64().map(Variant::Int64),
        (VariantType::Tags, Value::Array(items)) => {
            let mut tags = Tags::new();
            for item in items {
                tags.push(item.as_str().ok_or("tags are strings")?);
            }
            Some(Variant::Tags(tags))
        }
        (VariantType::MaterialColors, Value::Object(_)) => Some(Variant::MaterialColors(
            serde_json::from_value::<MaterialColors>(json.clone())
                .map_err(|e| format!("MaterialColors: {e}"))?,
        )),
        (VariantType::Attributes, Value::Object(_)) => Some(Variant::Attributes(attributes(json)?)),
        (VariantType::Vector2, _) => {
            numbers(json).map(|[x, y]| Variant::Vector2(Vector2::new(x as f32, y as f32)))
        }
        (VariantType::Vector2int16, _) => {
            numbers(json).map(|[x, y]| Variant::Vector2int16(Vector2int16::new(x as i16, y as i16)))
        }
        (VariantType::UDim, _) => {
            numbers(json).map(|[s, o]| Variant::UDim(UDim::new(s as f32, o as i32)))
        }
        (VariantType::NumberRange, _) => {
            numbers(json).map(|[a, b]| Variant::NumberRange(NumberRange::new(a as f32, b as f32)))
        }
        (VariantType::Vector3, _) => {
            numbers(json).map(|[x, y, z]| Variant::Vector3(vector3(x, y, z)))
        }
        (VariantType::Vector3int16, _) => numbers(json).map(|[x, y, z]| {
            Variant::Vector3int16(Vector3int16::new(x as i16, y as i16, z as i16))
        }),
        (VariantType::Color3, _) => numbers(json)
            .map(|[r, g, b]| Variant::Color3(Color3::new(r as f32, g as f32, b as f32))),
        (VariantType::Color3uint8, _) => numbers(json)
            .map(|[r, g, b]| Variant::Color3uint8(Color3uint8::new(r as u8, g as u8, b as u8))),
        (VariantType::UDim2, _) => numbers(json).map(|[xs, xo, ys, yo]| {
            Variant::UDim2(UDim2::new(
                UDim::new(xs as f32, xo as i32),
                UDim::new(ys as f32, yo as i32),
            ))
        }),
        (VariantType::Rect, _) => numbers(json).map(|[x0, y0, x1, y1]| {
            Variant::Rect(Rect::new(
                Vector2::new(x0 as f32, y0 as f32),
                Vector2::new(x1 as f32, y1 as f32),
            ))
        }),
        // Position, then the rotation matrix row by row.
        (VariantType::CFrame, _) => numbers::<12>(json).map(|m| Variant::CFrame(cframe(m))),
        (VariantType::OptionalCFrame, _) => {
            numbers::<12>(json).map(|m| Variant::OptionalCFrame(Some(cframe(m))))
        }
        _ => None,
    };
    v.ok_or_else(|| format!("{json} is not a {ty:?}"))
}

fn cframe(m: [f64; 12]) -> CFrame {
    CFrame::new(
        vector3(m[0], m[1], m[2]),
        Matrix3::new(
            vector3(m[3], m[4], m[5]),
            vector3(m[6], m[7], m[8]),
            vector3(m[9], m[10], m[11]),
        ),
    )
}

fn vector3(x: f64, y: f64, z: f64) -> Vector3 {
    Vector3::new(x as f32, y as f32, z as f32)
}
