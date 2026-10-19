//! Dependency layering rules, checked by `cargo xtask layers`.
//!
//! CaptureCraft reuses the storytold/photocraft crates (pinned git
//! dependencies, never vendored) and adds its own `capturecraft-*` crates
//! on top. Every crate, ours or theirs, sits on one layer; a crate may
//! depend only on lower layers (or on an earlier crate of an
//! [`INTRA_LAYER_ORDER`] chain).
//!
//! ```text
//!  L7  ui-egui ─┬─ automation (MCP + JSON control channel)      capturecraft, capturecraft-app, cli, xtask: exempt
//!   (photocraft-ui-egui: theme tokens, below ui-egui)
//!               │         │
//!  L6  capture ─┴─ ocr    │        OS capture (own CaptureBackend trait) / OCR (implements engine UiAnalyzer)
//!         │        │      │
//!  L5  format ───▶ engine ◀┘       .capturecraft archive + document model ▶ command registry, Session
//!         │        │
//!  L4  compositor                  CPU oracle + wgpu compositor (pixel parity)
//!         │
//!  L3  photocraft-vector  photocraft-text                      paths, booleans, strokes / shaping
//!         │                    │
//!  L2  photocraft-doc  photocraft-algo                         layer data / blur, mosaic, inpaint
//!         │                    │
//!  L1  photocraft-color ──▶ photocraft-raster                  blend math / 256² COW tiles
//!         │
//!  L0  photocraft-geom  photocraft-cms   [standalone: photocraft-codecs, photocraft-psd, pdfcraft-create]
//! ```
//!
//! The photocraft crates keep their own relative order (photocraft's
//! `xtask/src/layers.rs` puts `color`/`raster` at L0, `doc` at L1 and
//! `algo`/`text`/`vector` at L2); this table only spreads them over more
//! layers, so every edge their workspace allows is allowed here too.
//!
//! `capture` and `ocr` sit above `engine`. `capture` defines its own
//! `CaptureBackend` trait and produces documents (format, L5) that the app
//! opens in the engine; `ocr` implements the engine's `UiAnalyzer` trait,
//! which the app installs (photocraft does the same with its `platform`
//! crate). That keeps the engine, and everything it tests, free of OS code.
//!
//! The rule engine works on a small, metadata-independent model so it can be
//! unit-tested; [`from_metadata`] builds that model from `cargo metadata`.

use serde_json::Value;

/// Where a crate sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Regular layered crate.
    Layer(u8),
    /// Layer 0, and additionally may depend on no layered crate at all.
    Standalone,
    /// Binaries and build tooling: exempt from the rules, and no layered
    /// crate may depend on them.
    Exempt,
}

impl Class {
    fn layer(self) -> Option<u8> {
        match self {
            Class::Layer(l) => Some(l),
            Class::Standalone => Some(0),
            Class::Exempt => None,
        }
    }
}

/// The layering table, by full package name. Pinned external crates
/// (photocraft, pdfcraft) are listed too, so a workspace crate that pulls
/// one in is checked against its layer.
pub const TABLE: &[(&str, Class)] = &[
    // L0 foundation (photocraft, pinned by git tag)
    ("photocraft-geom", Class::Layer(0)),
    ("photocraft-cms", Class::Layer(0)),
    ("photocraft-codecs", Class::Standalone),
    ("photocraft-psd", Class::Standalone),
    // PDF export of documentation guides (storytold/pdfcraft).
    ("pdfcraft-create", Class::Standalone),
    // L1 pixels
    ("photocraft-color", Class::Layer(1)),
    ("photocraft-raster", Class::Layer(1)),
    // L2 layer data and imaging algorithms
    ("photocraft-doc", Class::Layer(2)),
    ("photocraft-algo", Class::Layer(2)),
    // L3 vector and type
    ("photocraft-vector", Class::Layer(3)),
    ("photocraft-text", Class::Layer(3)),
    // L4 compositing
    ("capturecraft-compositor", Class::Layer(4)),
    // L5 document model and command engine
    ("capturecraft-format", Class::Layer(5)),
    ("capturecraft-engine", Class::Layer(5)),
    // L6 OS capture and on-device analysis
    ("capturecraft-capture", Class::Layer(6)),
    ("capturecraft-ocr", Class::Layer(6)),
    // L7 thin UI and agent surfaces. photocraft-ui-egui supplies the
    // theme tokens shared with PhotoCraft's editor.
    ("photocraft-ui-egui", Class::Layer(7)),
    ("capturecraft-ui-egui", Class::Layer(7)),
    ("capturecraft-automation", Class::Layer(7)),
    // Apps and tooling
    ("capturecraft", Class::Exempt),
    ("capturecraft-app", Class::Exempt),
    ("capturecraft-cli", Class::Exempt),
    ("xtask", Class::Exempt),
];

/// Explicit orderings *within* a layer (earlier may be used by later).
pub const INTRA_LAYER_ORDER: &[&[&str]] = &[
    &["photocraft-geom", "photocraft-cms"],
    &["photocraft-color", "photocraft-raster"],
    &["photocraft-doc", "photocraft-algo"],
    &["photocraft-vector", "photocraft-text"],
    &["capturecraft-format", "capturecraft-engine"],
    &["photocraft-ui-egui", "capturecraft-ui-egui"],
];

fn intra_layer_allowed(from: &str, to: &str) -> bool {
    INTRA_LAYER_ORDER.iter().any(|chain| {
        match (
            chain.iter().position(|n| *n == from),
            chain.iter().position(|n| *n == to),
        ) {
            (Some(f), Some(t)) => t < f,
            _ => false,
        }
    })
}

/// External crates that make up a UI toolkit or windowing dependency.
/// Entries ending in `*` are prefixes.
pub const UI_CRATES: &[&str] = &[
    "egui*",
    "eframe",
    "winit",
    "rfd",
    "softbuffer",
    "tray-icon",
    "craft-ui",
];

/// First layer allowed to use UI crates.
pub const UI_MIN_LAYER: u8 = 7;

/// Prefixes of crate families that must be registered in [`TABLE`] before
/// a workspace crate may use them.
pub const KNOWN_FAMILIES: &[&str] = &["photocraft-", "pdfcraft-", "capturecraft-"];

pub fn classify(pkg: &str) -> Option<Class> {
    TABLE.iter().find(|(n, _)| *n == pkg).map(|(_, c)| *c)
}

fn is_ui_crate(name: &str) -> bool {
    UI_CRATES.iter().any(|p| match p.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == *p,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Clone)]
pub struct Dep {
    pub name: String,
    pub kind: DepKind,
}

#[derive(Debug, Clone)]
pub struct Crate {
    pub name: String,
    pub deps: Vec<Dep>,
}

/// Checks every workspace crate; returns one message per violation.
pub fn check(crates: &[Crate]) -> Vec<String> {
    let mut errors = Vec::new();
    for c in crates {
        let Some(class) = classify(&c.name) else {
            errors.push(format!(
                "{}: not registered in xtask/src/layers.rs TABLE",
                c.name
            ));
            continue;
        };
        let Some(layer) = class.layer() else {
            continue;
        };
        for d in &c.deps {
            if is_ui_crate(&d.name) && layer < UI_MIN_LAYER {
                errors.push(format!(
                    "{} (L{layer}) uses UI crate `{}`: only L{UI_MIN_LAYER} may",
                    c.name, d.name
                ));
                continue;
            }
            let Some(to) = classify(&d.name) else {
                if KNOWN_FAMILIES.iter().any(|f| d.name.starts_with(f)) {
                    errors.push(format!(
                        "{}: depends on unregistered `{}`; add it to the layer TABLE",
                        c.name, d.name
                    ));
                }
                continue;
            };
            if let Some(msg) = edge_error(&c.name, class, layer, &d.name, to, d.kind) {
                errors.push(msg);
            }
        }
    }
    errors
}

fn edge_error(
    from: &str,
    class: Class,
    layer: u8,
    to: &str,
    to_class: Class,
    kind: DepKind,
) -> Option<String> {
    let Some(to_layer) = to_class.layer() else {
        return Some(format!(
            "{from} depends on `{to}`, an app/tool crate (exempt crates are leaves)"
        ));
    };
    if class == Class::Standalone {
        return Some(format!("{from} is standalone and may not depend on `{to}`"));
    }
    let ok = match kind {
        // Tests may use anything at or below their own layer.
        DepKind::Dev => to_layer <= layer,
        DepKind::Normal | DepKind::Build => {
            to_layer < layer || (to_layer == layer && intra_layer_allowed(from, to))
        }
    };
    (!ok).then(|| {
        let kind = match kind {
            DepKind::Normal => "",
            DepKind::Dev => " (dev)",
            DepKind::Build => " (build)",
        };
        format!("{from} (L{layer}) depends{kind} on `{to}` (L{to_layer}): layers point down only")
    })
}

/// Builds the model from `cargo metadata --format-version 1 --no-deps`
/// output (workspace members only; their declared deps include externals).
pub fn from_metadata(meta: &Value) -> Result<Vec<Crate>, String> {
    let packages = meta
        .get("packages")
        .and_then(Value::as_array)
        .ok_or("cargo metadata: no `packages` array")?;
    let mut crates = Vec::with_capacity(packages.len());
    for p in packages {
        let name = p
            .get("name")
            .and_then(Value::as_str)
            .ok_or("cargo metadata: package without a name")?;
        let deps = p
            .get("dependencies")
            .and_then(Value::as_array)
            .map(|deps| deps.iter().filter_map(dep_from_metadata).collect())
            .unwrap_or_default();
        crates.push(Crate {
            name: name.to_string(),
            deps,
        });
    }
    Ok(crates)
}

fn dep_from_metadata(d: &Value) -> Option<Dep> {
    let name = d.get("name").and_then(Value::as_str)?;
    let kind = match d.get("kind").and_then(Value::as_str) {
        Some("dev") => DepKind::Dev,
        Some("build") => DepKind::Build,
        _ => DepKind::Normal,
    };
    Some(Dep {
        name: name.to_string(),
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn krate(name: &str, deps: &[(&str, DepKind)]) -> Crate {
        Crate {
            name: name.into(),
            deps: deps
                .iter()
                .map(|(n, k)| Dep {
                    name: (*n).into(),
                    kind: *k,
                })
                .collect(),
        }
    }

    #[test]
    fn downward_edges_pass() {
        let c = [
            krate(
                "capturecraft-engine",
                &[
                    ("capturecraft-format", DepKind::Normal),
                    ("photocraft-algo", DepKind::Normal),
                    ("serde_json", DepKind::Normal),
                ],
            ),
            krate("capturecraft-ui-egui", &[("egui", DepKind::Normal)]),
            krate("capturecraft", &[("capturecraft-ui-egui", DepKind::Normal)]),
        ];
        assert_eq!(check(&c), Vec::<String>::new());
    }

    #[test]
    fn upward_and_reverse_intra_edges_fail() {
        let c = [
            krate(
                "capturecraft-engine",
                &[("capturecraft-capture", DepKind::Normal)],
            ),
            krate(
                "capturecraft-format",
                &[("capturecraft-engine", DepKind::Normal)],
            ),
        ];
        assert_eq!(check(&c).len(), 2);
    }

    #[test]
    fn ui_crates_only_at_the_top() {
        let c = [krate("capturecraft-engine", &[("eframe", DepKind::Dev)])];
        assert_eq!(check(&c).len(), 1);
        let c = [krate(
            "capturecraft-compositor",
            &[("egui_extras", DepKind::Normal)],
        )];
        assert_eq!(check(&c).len(), 1);
    }

    #[test]
    fn unregistered_crates_fail() {
        let c = [
            krate("capturecraft-mystery", &[]),
            krate(
                "capturecraft-format",
                &[("photocraft-paint", DepKind::Normal)],
            ),
        ];
        assert_eq!(check(&c).len(), 2);
    }

    #[test]
    fn standalone_and_exempt_rules() {
        let c = [krate(
            "pdfcraft-create",
            &[("photocraft-geom", DepKind::Normal)],
        )];
        assert_eq!(check(&c).len(), 1);
        let c = [krate(
            "capturecraft-engine",
            &[("capturecraft-cli", DepKind::Normal)],
        )];
        assert_eq!(check(&c).len(), 1);
    }

    #[test]
    fn dev_deps_may_stay_on_their_layer() {
        let c = [krate(
            "capturecraft-format",
            &[("capturecraft-engine", DepKind::Dev)],
        )];
        assert!(check(&c).is_empty());
    }

    #[test]
    fn parses_metadata() {
        let meta = serde_json::json!({ "packages": [{
            "name": "capturecraft-format",
            "dependencies": [
                { "name": "photocraft-geom", "kind": null },
                { "name": "serde_json", "kind": "dev" }
            ]
        }]});
        let crates = from_metadata(&meta).unwrap_or_default();
        assert_eq!(crates.len(), 1);
        assert_eq!(crates[0].deps[1].kind, DepKind::Dev);
    }
}
