//! Exported script properties: values a scene file (or the inspector) sets on a script
//! instance, kept as TOML and handed to the script as [`ffi::Value`]s of the declared kind.

use ab_script_api as ffi;
use toml::Value as TomlValue;

/// A property a script type exports, as the inspector shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptPropertyDesc {
    pub name: String,
    pub kind: ffi::Tag,
    /// The script's default, in the form scene files store it.
    pub default: Option<TomlValue>,
    /// Bit 0: hidden from the inspector.
    pub flags: u32,
    pub doc: String,
}

impl ScriptPropertyDesc {
    pub const HIDDEN: u32 = 1;

    /// Reads a library's description of a property.
    ///
    /// # Safety
    /// The strings of `desc` (and of a string default) must be valid.
    pub unsafe fn from_ffi(desc: &ffi::PropertyDesc) -> Self {
        Self {
            // SAFETY: see above.
            name: unsafe { desc.name.to_str() }.unwrap_or("").to_owned(),
            kind: desc.kind,
            // SAFETY: see above.
            default: unsafe { to_toml(&desc.default_value) },
            flags: desc.flags,
            // SAFETY: see above.
            doc: unsafe { desc.doc.to_str() }.unwrap_or("").to_owned(),
        }
    }

    pub fn hidden(&self) -> bool {
        self.flags & Self::HIDDEN != 0
    }
}

/// `value` in the form scene files store it: numbers, arrays for vectors and quaternions,
/// `"#rrggbbaa"` (sRGB) for colors. `None` for nil and nodes.
///
/// # Safety
/// A string value must point to valid UTF-8 (or be invalid as a whole: it then reads as
/// `None`).
pub unsafe fn to_toml(value: &ffi::Value) -> Option<TomlValue> {
    let floats =
        |v: &[f32]| TomlValue::Array(v.iter().map(|&x| TomlValue::Float(f64::from(x))).collect());
    Some(match value.tag {
        ffi::Tag::BOOL => TomlValue::Boolean(value.as_bool()?),
        ffi::Tag::INT => TomlValue::Integer(value.as_int()?),
        ffi::Tag::FLOAT => TomlValue::Float(value.as_float()?),
        ffi::Tag::VEC2 => {
            let v = value.as_vec2()?;
            floats(&[v.x, v.y])
        }
        ffi::Tag::VEC3 => {
            let v = value.as_vec3()?;
            floats(&[v.x, v.y, v.z])
        }
        ffi::Tag::QUAT => {
            let q = value.as_quat()?;
            floats(&[q.x, q.y, q.z, q.w])
        }
        ffi::Tag::COLOR => TomlValue::String(color_hex(value.as_color()?)),
        // SAFETY: the caller vouches for the string.
        ffi::Tag::STR => TomlValue::String(unsafe { value.as_str() }?.to_owned()),
        _ => return None,
    })
}

/// `value` as a property of `kind`, converting where that is lossless or expected (an
/// integer for a float, integers in a vector, a hex string or an array for a color).
/// `kind` `None` (no description of the script) guesses from the value. A string result
/// borrows `value`'s text: use it before `value` goes away.
pub fn to_ffi(kind: Option<ffi::Tag>, value: &TomlValue) -> Option<ffi::Value> {
    let number = |v: &TomlValue| match v {
        TomlValue::Float(f) => Some(*f),
        TomlValue::Integer(i) => Some(*i as f64),
        _ => None,
    };
    let numbers = |len: usize| -> Option<Vec<f32>> {
        let a = value.as_array().filter(|a| a.len() == len)?;
        a.iter().map(|v| number(v).map(|f| f as f32)).collect()
    };
    let kind = match kind {
        Some(kind) => kind,
        None => match value {
            TomlValue::Boolean(_) => ffi::Tag::BOOL,
            TomlValue::Integer(_) => ffi::Tag::INT,
            TomlValue::Float(_) => ffi::Tag::FLOAT,
            TomlValue::String(_) => ffi::Tag::STR,
            TomlValue::Array(a) => match a.len() {
                2 => ffi::Tag::VEC2,
                3 => ffi::Tag::VEC3,
                4 => ffi::Tag::QUAT,
                _ => return None,
            },
            _ => return None,
        },
    };
    Some(match kind {
        ffi::Tag::BOOL => ffi::Value::bool(value.as_bool()?),
        ffi::Tag::INT => match value {
            TomlValue::Integer(i) => ffi::Value::int(*i),
            // A whole float (`3.0`) is fine; `2.5` is not an integer.
            TomlValue::Float(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => {
                ffi::Value::int(*f as i64)
            }
            _ => return None,
        },
        ffi::Tag::FLOAT => ffi::Value::float(number(value)?),
        ffi::Tag::VEC2 => {
            let v = numbers(2)?;
            ffi::Value::vec2(ffi::Vec2 { x: v[0], y: v[1] })
        }
        ffi::Tag::VEC3 => {
            let v = numbers(3)?;
            ffi::Value::vec3(ffi::Vec3 {
                x: v[0],
                y: v[1],
                z: v[2],
            })
        }
        ffi::Tag::QUAT => {
            let v = numbers(4)?;
            ffi::Value::quat(ffi::Quat {
                x: v[0],
                y: v[1],
                z: v[2],
                w: v[3],
            })
        }
        ffi::Tag::COLOR => ffi::Value::color(match value {
            TomlValue::String(hex) => parse_color_hex(hex)?,
            _ => {
                let v = numbers(4).or_else(|| {
                    numbers(3).map(|mut v| {
                        v.push(1.0);
                        v
                    })
                })?;
                ffi::Color {
                    r: v[0],
                    g: v[1],
                    b: v[2],
                    a: v[3],
                }
            }
        }),
        ffi::Tag::STR => ffi::Value::str(ffi::Str::new(value.as_str()?)),
        _ => return None,
    })
}

/// A linear color as scene files write colors: `#rrggbbaa`, sRGB.
pub fn color_hex(c: ffi::Color) -> String {
    let encode = |c: f32| {
        let c = c.clamp(0.0, 1.0);
        let s = if c <= 0.003_130_8 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0).round() as u8
    };
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        encode(c.r),
        encode(c.g),
        encode(c.b),
        (c.a.clamp(0.0, 1.0) * 255.0).round() as u8
    )
}

/// `#rrggbb` or `#rrggbbaa` (sRGB, the `#` optional) as a linear color.
pub fn parse_color_hex(hex: &str) -> Option<ffi::Color> {
    let digits = hex.strip_prefix('#').unwrap_or(hex);
    let byte = |i: usize| {
        digits
            .get(i..i + 2)
            .and_then(|s| u8::from_str_radix(s, 16).ok())
    };
    let decode = |b: u8| {
        let s = f32::from(b) / 255.0;
        if s <= 0.040_45 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    let alpha = match digits.len() {
        6 => 255,
        8 => byte(6)?,
        _ => return None,
    };
    Some(ffi::Color {
        r: decode(byte(0)?),
        g: decode(byte(2)?),
        b: decode(byte(4)?),
        a: f32::from(alpha) / 255.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_by_declared_kind() {
        // `speed = 5` in a file is a float property of 5.0, not an integer.
        let v = to_ffi(Some(ffi::Tag::FLOAT), &TomlValue::Integer(5)).unwrap();
        assert_eq!(v.tag, ffi::Tag::FLOAT);
        assert_eq!(v.as_float(), Some(5.0));
        assert!(to_ffi(Some(ffi::Tag::INT), &TomlValue::Float(2.5)).is_none());
        // Mixed integers and floats in a vector.
        let mixed = TomlValue::Array(vec![TomlValue::Integer(1), TomlValue::Float(2.5)]);
        let v = to_ffi(Some(ffi::Tag::VEC2), &mixed)
            .unwrap()
            .as_vec2()
            .unwrap();
        assert_eq!((v.x, v.y), (1.0, 2.5));
        // A color is a color, not a string or a quaternion.
        let red = to_ffi(Some(ffi::Tag::COLOR), &TomlValue::String("#ff0000".into())).unwrap();
        assert_eq!(red.tag, ffi::Tag::COLOR);
        let four = TomlValue::Array(vec![TomlValue::Float(1.0); 4]);
        assert_eq!(
            to_ffi(Some(ffi::Tag::COLOR), &four).unwrap().tag,
            ffi::Tag::COLOR
        );
        // Wrong types are refused instead of reaching the script.
        assert!(to_ffi(Some(ffi::Tag::BOOL), &TomlValue::Integer(1)).is_none());
        assert!(to_ffi(Some(ffi::Tag::VEC3), &mixed).is_none());
    }

    #[test]
    fn colors_round_trip_through_hex() {
        for hex in ["#ff8040ff", "#00000080", "#123456ff"] {
            let c = parse_color_hex(hex).unwrap();
            assert_eq!(color_hex(c), hex);
        }
        assert!(parse_color_hex("#12345").is_none());
        assert!(parse_color_hex("#zzzzzz").is_none());
    }

    #[test]
    fn defaults_are_stored_as_files_store_them() {
        // SAFETY: no strings, or strings that outlive the calls.
        let one = unsafe { to_toml(&ffi::Value::float(1.0)) }.unwrap();
        assert_eq!(one, TomlValue::Float(1.0));
        let text = "hello";
        // SAFETY: `text` outlives the call.
        let s = unsafe { to_toml(&ffi::Value::str(ffi::Str::new(text))) }.unwrap();
        assert_eq!(s.as_str(), Some("hello"));
        // SAFETY: no string.
        assert!(unsafe { to_toml(&ffi::Value::NIL) }.is_none());
    }
}
