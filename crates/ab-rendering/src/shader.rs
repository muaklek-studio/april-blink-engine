//! Custom surface shaders.
//!
//! A custom shader is a WGSL file with a `surface` function. The forward shader works out the
//! surface as usual (colour, textures, normal map, metallic, roughness) and passes it to the
//! function, which returns it changed; lighting, shadows and fog then work as for any
//! material:
//!
//! ```wgsl
//! // param speed = 1.0
//! // param strength = 0.5
//!
//! fn surface(input: SurfaceInput, base: Surface) -> Surface {
//!     var s = base;
//!     let wave = sin(input.world_position.x * 4.0 + input.time * input.params.speed);
//!     s.emission += vec3<f32>(0.2, 0.6, 1.0) * max(wave, 0.0) * input.params.strength;
//!     return s;
//! }
//! ```
//!
//! `// param <name> = <default>` lines declare up to [`MAX_PARAMS`] numbers a material sets
//! (`input.params.<name>`). `SurfaceInput` has `world_position`, `normal` (the mesh's), `uv`,
//! `view` (towards the camera), `time` (seconds) and `front_facing`; `Surface` has
//! `albedo`, `alpha` (cut out below the material's alpha cutoff), `emission`, `metallic`,
//! `roughness`, `normal` (world space) and `unshaded`. The shader may use the forward
//! shader's functions and the material's textures (`albedo_texture`, `albedo_sampler`, …).
//!
//! [`validate`] checks a shader as the renderer will build it and reports errors at the
//! file's own lines; the renderer draws a material whose shader does not build with the
//! standard surface instead.

use std::fmt;

/// Numbers a custom shader can declare (`// param`).
pub const MAX_PARAMS: usize = 8;

/// Reference to a shader in [`RenderAssets`](crate::RenderAssets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShaderHandle(pub(crate) u32);

impl ShaderHandle {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A custom shader's code, and its revision (bumped when the code changes, so the renderer
/// builds it again).
#[derive(Debug, Clone, PartialEq)]
pub struct ShaderCode {
    pub name: String,
    pub code: String,
    pub revision: u64,
}

/// A number a shader declares.
#[derive(Debug, Clone, PartialEq)]
pub struct ShaderParam {
    pub name: String,
    pub default: f32,
}

/// A shader error, at a line of the shader's own file (when it is in it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShaderError {
    /// 1-based, in the custom shader's code.
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub message: String,
}

impl fmt::Display for ShaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.line, self.column) {
            (Some(line), Some(column)) => write!(f, "line {line}:{column}: {}", self.message),
            (Some(line), None) => write!(f, "line {line}: {}", self.message),
            _ => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ShaderError {}

/// The code a new shader file starts with.
pub const TEMPLATE: &str = r#"// A custom surface: change what the standard shader worked out, before lighting.
// `// param name = default` lines are numbers each material sets (input.params.name).

// param speed = 1.0
// param glow = 0.5

fn surface(input: SurfaceInput, base: Surface) -> Surface {
    var s = base;
    // A band of light moving up the surface.
    let band = fract(input.world_position.y * 0.5 - input.time * input.params.speed * 0.25);
    s.emission += base.albedo * smoothstep(0.9, 1.0, band) * input.params.glow * 4.0;
    return s;
}
"#;

/// The standard surface: unchanged.
const DEFAULT_SURFACE: &str =
    "fn surface(input: SurfaceInput, base: Surface) -> Surface {\n    return base;\n}\n";

const MARKER: &str = "//@SURFACE\n";

/// The fog and forward shaders (the custom surface goes at the marker).
pub(crate) fn forward_template() -> String {
    concat!(
        include_str!("shaders/fog.wgsl"),
        include_str!("shaders/forward.wgsl")
    )
    .to_owned()
}

/// The `// param name = default` lines of `code`, in order (at most [`MAX_PARAMS`]; a
/// repeated name counts once).
pub fn params(code: &str) -> Vec<ShaderParam> {
    let mut out: Vec<ShaderParam> = Vec::new();
    for line in code.lines() {
        let Some(rest) = line.trim().strip_prefix("//") else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix("param ") else {
            continue;
        };
        let (name, default) = match rest.split_once('=') {
            Some((name, value)) => (name.trim(), value.trim().parse().unwrap_or(0.0)),
            None => (rest.trim(), 0.0),
        };
        let identifier = name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if identifier && out.len() < MAX_PARAMS && !out.iter().any(|p| p.name == name) {
            out.push(ShaderParam {
                name: name.to_owned(),
                default,
            });
        }
    }
    out
}

/// The full forward shader with `code`'s surface (`None`: the standard one), and the line
/// `code` starts at in it.
pub fn compose(code: Option<&str>) -> (String, u32) {
    let template = forward_template();
    let declared = code.map(params).unwrap_or_default();
    let mut generated = String::from("struct Params {\n");
    if declared.is_empty() {
        generated.push_str("    unused: f32,\n");
    }
    for p in &declared {
        generated.push_str(&format!("    {}: f32,\n", p.name));
    }
    generated.push_str("};\n\nfn shader_params(m: Material) -> Params {\n    return Params(");
    let values: Vec<String> = if declared.is_empty() {
        vec!["0.0".into()]
    } else {
        (0..declared.len())
            .map(|i| format!("m.custom{}.{}", i / 4, ["x", "y", "z", "w"][i % 4]))
            .collect()
    };
    generated.push_str(&values.join(", "));
    generated.push_str(");\n}\n\n");
    let (before, after) = template
        .split_once(MARKER)
        .expect("forward.wgsl has the surface marker");
    let first_line = (before.lines().count() + generated.lines().count() + 1) as u32;
    let mut code = code.unwrap_or(DEFAULT_SURFACE).to_owned();
    if !code.ends_with('\n') {
        code.push('\n');
    }
    (format!("{before}{generated}{code}{after}"), first_line)
}

/// Checks that `code` builds as a custom surface.
pub fn validate(code: &str) -> Result<(), ShaderError> {
    let (source, first_line) = compose(Some(code));
    let user_lines = code.lines().count().max(1) as u32;
    let locate = |location: Option<naga::SourceLocation>, message: String| {
        let line = location
            .map(|l| l.line_number)
            .filter(|&l| l >= first_line && l < first_line + user_lines);
        ShaderError {
            line: line.map(|l| l - first_line + 1),
            column: line.and(location.map(|l| l.line_position)),
            message,
        }
    };
    let defines_surface = code.lines().any(|line| {
        let line = line.trim_start();
        line.strip_prefix("fn").is_some_and(|rest| {
            rest.trim_start()
                .strip_prefix("surface")
                .is_some_and(|r| r.trim_start().starts_with('('))
        })
    });
    if !defines_surface {
        return Err(ShaderError {
            line: None,
            column: None,
            message:
                "the shader has no `fn surface(input: SurfaceInput, base: Surface) -> Surface`"
                    .into(),
        });
    }
    let module = naga::front::wgsl::parse_str(&source)
        .map_err(|e| locate(e.location(&source), e.message().to_owned()))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| {
        let mut message = e.as_inner().to_string();
        let mut source_error = std::error::Error::source(e.as_inner());
        while let Some(inner) = source_error {
            message.push_str(": ");
            message.push_str(&inner.to_string());
            source_error = inner.source();
        }
        // The narrowest span: the statement, not the whole function.
        let span = e
            .spans()
            .map(|(span, _)| *span)
            .filter(|span| span.is_defined())
            .min_by_key(|span| span.to_range().map_or(usize::MAX, |r| r.len()));
        locate(span.map(|s| s.location(&source)), message)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standard_and_template_shaders_build() {
        let (source, _) = compose(None);
        naga::front::wgsl::parse_str(&source).unwrap();
        validate(DEFAULT_SURFACE).unwrap();
        validate(TEMPLATE).unwrap();
        assert_eq!(
            params(TEMPLATE),
            vec![
                ShaderParam {
                    name: "speed".into(),
                    default: 1.0
                },
                ShaderParam {
                    name: "glow".into(),
                    default: 0.5
                },
            ]
        );
    }

    #[test]
    fn errors_point_at_the_shaders_own_lines() {
        let code = "// param k = 2\n\nfn surface(input: SurfaceInput, base: Surface) -> Surface {\n    var s = base;\n    s.albedo = 1.0;\n    return s;\n}\n";
        let err = validate(code).unwrap_err();
        assert_eq!(err.line, Some(5), "{err}");
        let err = validate(
            "fn surface(input: SurfaceInput, base: Surface) -> Surface {\n    return bas;\n}\n",
        )
        .unwrap_err();
        assert_eq!(err.line, Some(2), "{err}");
        assert!(err.message.contains("bas"), "{err}");
        let err = validate("fn other() {}\n").unwrap_err();
        assert!(err.message.contains("no `fn surface"), "{err}");
    }

    #[test]
    fn params_are_parsed_in_order_and_capped() {
        let code = (0..12)
            .map(|i| format!("// param p{i} = {i}.5\n"))
            .collect::<String>()
            + "// param p0 = 9\n//param bad-name = 1\n// param bare\n";
        let found = params(&code);
        assert_eq!(found.len(), MAX_PARAMS);
        assert_eq!(
            found[3],
            ShaderParam {
                name: "p3".into(),
                default: 3.5
            }
        );
        let (source, _) = compose(Some(&code));
        assert!(source.contains("m.custom1.w"));
    }
}
