//! One model file: what it declares, the textures it needs, and the template every
//! instance is cloned from.
//!
//! Loading happens in two phases on purpose. [`Asset::parse`] reads and parses
//! everything without touching the GPU, which is enough to validate a host's
//! interface against the model — so a model that does not expose the parameters an
//! interface names fails before a single texel is uploaded. [`Prepared::upload`]
//! is the only phase that allocates textures.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mocari::assets::{RuntimeModel, load_model_runtime};
use mocari::json::Physics3;
use mocari::render::wgpu::{WgpuLive2dRenderer, WgpuTexture};
use mocari::ModelRuntime;
use wgpu::{Device, Queue};

use crate::bounds::ModelBounds;
use crate::error::{Error, Result};
use crate::info::{AssetId, HitArea, ModelInfo, MotionGroup, ParameterRange};
use crate::model::Model;
use crate::rig::{self, LookAtMode, ModelFacts, Rig};

/// The group name a model's own idle motions conventionally live in.
///
/// A constant, not a policy: this crate never picks a group to play.
pub const IDLE_GROUP: &str = "Idle";

/// Group name given to motions discovered by scanning the model directory.
///
/// Only used when [`AssetOptions::scan_sidecars`] is set; the clips keep the
/// alphabetical order of their files.
pub const SCANNED_GROUP: &str = "Scanned";

/// Longest directory scan, in levels below the model file.
const MAX_SCAN_DEPTH: usize = 2;

/// What one asset may spend on textures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AssetOptions {
    /// Texture resolution multiplier applied when uploading.
    ///
    /// 1.0 keeps the authored resolution (a 5-texture rig with one 8192x8192
    /// atlas costs ~592 MB of VRAM). 0.5 quarters that, which is plenty for a
    /// few-hundred-pixel desktop pet.
    pub texture_scale: f32,
    /// Whether to look for `.motion3.json` / `.exp3.json` files next to the model
    /// when `model3.json` declares none of its own.
    ///
    /// Off by default: a directory full of files is not the same thing as an
    /// animation set, and only the host knows what the files are. What is found
    /// lands in [`SCANNED_GROUP`], in the alphabetical order of the files.
    pub scan_sidecars: bool,
}

impl Default for AssetOptions {
    fn default() -> Self {
        Self {
            texture_scale: 0.5,
            scan_sidecars: false,
        }
    }
}

/// What one instance may spend on GPU memory.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelOptions {
    /// Edge length of the instance's mask render target. Raise it if a complex
    /// rig shows clipped geometry — packed mask contexts can run out of atlas
    /// space.
    pub mask_size: u32,
}

impl Default for ModelOptions {
    fn default() -> Self {
        Self { mask_size: 1024 }
    }
}

/// What every instance of one asset shares.
#[derive(Debug, Clone)]
pub(crate) struct Shared {
    pub info: Arc<ModelInfo>,
    /// Motion files per group, in model order.
    pub motions: Arc<[Arc<[PathBuf]>]>,
    pub expressions: Arc<[ExpressionRef]>,
    pub rigs: Arc<Rigs>,
}

#[derive(Debug)]
#[derive(Clone)]
pub(crate) struct ExpressionRef {
    pub name: Arc<str>,
    pub path: PathBuf,
}

/// The look-at table resolved once per mode.
#[derive(Debug)]
pub(crate) struct Rigs {
    adapted: Rig,
    viewer: Rig,
}

impl Rigs {
    pub fn get(&self, mode: LookAtMode) -> &Rig {
        match mode {
            LookAtMode::Adapted => &self.adapted,
            LookAtMode::ViewerDefault => &self.viewer,
        }
    }
}

/// A parsed model that has not reached the GPU yet.
#[derive(Debug)]
pub struct Prepared {
    shared: Shared,
    template: ModelRuntime,
    loaded: RuntimeModel,
    issues: Vec<Error>,
}

/// A loaded model: textures, metadata, and the template every instance clones.
///
/// One asset can back any number of [`Model`] instances; they share the textures
/// and the parsed data, and keep separate parameter, playback and physics state.
pub struct Asset {
    shared: Shared,
    template: ModelRuntime,
    textures: Vec<WgpuTexture>,
    issues: Vec<Error>,
}

impl Asset {
    /// Reads and parses `path` (a `.model3.json`) without touching the GPU.
    ///
    /// `id` is the identity every key minted from this asset will carry; derive
    /// it with [`AssetId::unique`]. Only [`AssetOptions::scan_sidecars`] is read
    /// here; the texture budget is used when the asset is uploaded.
    pub fn parse(id: AssetId, path: impl AsRef<Path>, options: AssetOptions) -> Result<Prepared> {
        let loaded = load_model_runtime(path.as_ref())?;
        let model_dir = loaded.model_dir().map(Path::to_path_buf);
        let runtime = loaded.runtime();

        let bounds = ModelBounds::from_canvas(&runtime.canvas())
            .or_else(|| ModelBounds::from_drawables(runtime.meshes()))
            .ok_or(Error::NoDrawables)?;

        let mut issues = Vec::new();
        let mut motion_groups = motion_paths_by_group(runtime, model_dir.as_deref());
        let mut expressions = expression_paths(runtime, model_dir.as_deref());

        // A directory full of files is not an animation set: what a clip means is
        // the host's judgement, so it has to ask for the scan.
        if options.scan_sidecars
            && let Some(dir) = model_dir.as_deref()
            && (motion_groups.is_empty() || expressions.is_empty())
        {
            let scanned = scan_sidecars(dir, &mut issues);

            if motion_groups.is_empty() && !scanned.motions.is_empty() {
                motion_groups.insert(SCANNED_GROUP.to_owned(), scanned.motions);
            }

            if expressions.is_empty() {
                expressions = scanned.expressions;
            }
        }

        let physics_outputs = physics_output_parameters(runtime, model_dir.as_deref(), &mut issues);
        let facts = ModelFacts::new(runtime.parameter_ids().to_vec(), physics_outputs);
        let rigs = Rigs {
            adapted: rig::resolve(LookAtMode::Adapted, &facts),
            viewer: rig::resolve(LookAtMode::ViewerDefault, &facts),
        };

        let motion_groups: Vec<(String, Vec<PathBuf>)> = motion_groups.into_iter().collect();
        let info = Arc::new(ModelInfo {
            asset: id,
            bounds,
            parameters: runtime
                .parameter_infos()
                .map(|parameter| ParameterRange {
                    name: Arc::from(parameter.id()),
                    minimum: parameter.minimum(),
                    maximum: parameter.maximum(),
                    default: parameter.default(),
                })
                .collect(),
            parts: runtime
                .part_ids()
                .iter()
                .map(|part| Arc::from(part.as_str()))
                .collect(),
            motions: motion_groups
                .iter()
                .map(|(name, clips)| MotionGroup {
                    name: Arc::from(name.as_str()),
                    clips: clips.len(),
                })
                .collect(),
            expressions: expressions.iter().map(|item| Arc::clone(&item.name)).collect(),
            hit_areas: runtime
                .model()
                .hit_areas()
                .iter()
                .map(|hit_area| HitArea {
                    id: Arc::from(hit_area.id()),
                    name: Arc::from(hit_area.name()),
                })
                .collect(),
        });

        // Cloning the runtime and dropping `loaded`'s handle frees the decoded PNG
        // bytes once they are on the GPU; five 4096x4096 textures is 335 MB of CPU
        // memory we never need again.
        let template = runtime.clone();

        Ok(Prepared {
            shared: Shared {
                info,
                motions: motion_groups
                    .into_iter()
                    .map(|(_, clips)| clips.into())
                    .collect(),
                expressions: expressions.into(),
                rigs: Arc::new(rigs),
            },
            template,
            loaded,
            issues,
        })
    }
}

impl Prepared {
    /// What the model declares, for interface validation before the upload.
    pub fn info(&self) -> &Arc<ModelInfo> {
        &self.shared.info
    }

    /// Problems found while parsing, which do not stop the model from drawing.
    pub fn issues(&self) -> &[Error] {
        &self.issues
    }

    /// Uploads the textures and keeps the result as the shared asset.
    pub fn upload(
        self,
        renderer: &WgpuLive2dRenderer,
        device: &Device,
        queue: &Queue,
        options: AssetOptions,
    ) -> Result<Asset> {
        let factor = downscale_factor(options.texture_scale);

        let textures = self
            .loaded
            .textures()
            .iter()
            .map(|texture| {
                let (width, height, rgba) = if factor > 1 {
                    downscale_rgba(texture.width(), texture.height(), texture.rgba(), factor)
                } else {
                    (texture.width(), texture.height(), texture.rgba().to_vec())
                };

                renderer
                    .create_rgba8_texture(device, queue, width, height, &rgba)
                    .map_err(Error::from)
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Asset {
            shared: self.shared,
            template: self.template,
            textures,
            issues: self.issues,
        })
    }
}

impl Asset {
    /// What the model declares.
    pub fn info(&self) -> &Arc<ModelInfo> {
        &self.shared.info
    }

    /// Problems found while loading, which do not stop the model from drawing.
    ///
    /// Takes them out: each one is worth reporting once.
    pub fn take_issues(&mut self) -> Vec<Error> {
        std::mem::take(&mut self.issues)
    }

    /// Creates an independent instance of this asset.
    ///
    /// The instance clones the parsed model (its own parameter values, physics
    /// state and playback position) and shares the textures.
    pub fn model(&self) -> Model {
        Model::new(self.template.clone(), self.shared.clone())
    }

    /// The uploaded textures, in drawable texture-index order.
    pub fn textures(&self) -> &[WgpuTexture] {
        &self.textures
    }
}

fn motion_paths_by_group(
    runtime: &ModelRuntime,
    model_dir: Option<&Path>,
) -> BTreeMap<String, Vec<PathBuf>> {
    let Some(model_dir) = model_dir else {
        return BTreeMap::new();
    };

    runtime
        .model()
        .motions()
        .iter()
        .map(|(group, references)| {
            (
                group.clone(),
                references
                    .iter()
                    .map(|reference| model_dir.join(reference.file()))
                    .collect(),
            )
        })
        .collect()
}

fn expression_paths(runtime: &ModelRuntime, model_dir: Option<&Path>) -> Vec<ExpressionRef> {
    let Some(model_dir) = model_dir else {
        return Vec::new();
    };

    runtime
        .model()
        .expressions()
        .iter()
        .map(|reference| ExpressionRef {
            name: Arc::from(reference.name()),
            path: model_dir.join(reference.file()),
        })
        .collect()
}

struct ScannedSidecars {
    motions: Vec<PathBuf>,
    expressions: Vec<ExpressionRef>,
}

/// Every parameter a physics setting writes, i.e. the ones a host must not drive.
///
/// Read from the `physics3.json` the model references. A missing or unparsable
/// file yields an empty set — the resolver then falls back to the conventional
/// names — but it is reported rather than silently ignored.
fn physics_output_parameters(
    model: &ModelRuntime,
    model_dir: Option<&Path>,
    issues: &mut Vec<Error>,
) -> Vec<String> {
    let (Some(dir), Some(file)) = (model_dir, model.model().physics()) else {
        return Vec::new();
    };

    let path = dir.join(file);

    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(source) => {
            issues.push(Error::PhysicsFile { path, source });
            return Vec::new();
        }
    };

    let physics = match Physics3::from_json_str(&source) {
        Ok(physics) => physics,
        Err(_) => {
            issues.push(Error::PhysicsParse { path });
            return Vec::new();
        }
    };

    physics
        .settings()
        .iter()
        .flat_map(|setting| setting.outputs())
        .map(|output| output.destination().id().to_owned())
        .collect()
}

/// Walks the model directory looking for `.motion3.json` / `.exp3.json` files.
///
/// Unreadable directories are pushed into `issues` rather than skipped: a scan
/// that silently finds nothing looks exactly like a model with no animations.
fn scan_sidecars(dir: &Path, issues: &mut Vec<Error>) -> ScannedSidecars {
    let mut motions = Vec::new();
    let mut expressions = Vec::new();
    collect_sidecars(dir, 0, &mut motions, &mut expressions, issues);

    motions.sort();
    expressions.sort();

    ScannedSidecars {
        motions,
        expressions: expressions
            .into_iter()
            .map(|path| ExpressionRef {
                name: Arc::from(
                    path.file_stem()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default()
                        .trim_end_matches(".exp3"),
                ),
                path,
            })
            .collect(),
    }
}

fn collect_sidecars(
    dir: &Path,
    depth: usize,
    motions: &mut Vec<PathBuf>,
    expressions: &mut Vec<PathBuf>,
    issues: &mut Vec<Error>,
) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(source) => {
            issues.push(Error::Scan {
                path: dir.to_path_buf(),
                source,
            });
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            collect_sidecars(&path, depth + 1, motions, expressions, issues);
            continue;
        }

        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        let lower = name.to_ascii_lowercase();

        if lower.ends_with(".motion3.json") {
            motions.push(path);
        } else if lower.ends_with(".exp3.json") {
            expressions.push(path);
        }
    }
}

/// Integer box-filter factor derived from a `0..1` texture scale.
fn downscale_factor(scale: f32) -> u32 {
    if !scale.is_finite() || scale >= 0.999 {
        return 1;
    }

    ((1.0 / scale).round() as u32).max(1)
}

/// Box-filtered integer downscale.
///
/// Averaging happens in premultiplied space and is then un-premultiplied, so
/// texels that neighbour fully transparent pixels do not pick up dark halos.
fn downscale_rgba(width: u32, height: u32, rgba: &[u8], factor: u32) -> (u32, u32, Vec<u8>) {
    let factor = factor.max(1);

    if factor == 1 {
        return (width, height, rgba.to_vec());
    }

    let out_width = (width / factor).max(1);
    let out_height = (height / factor).max(1);
    let mut out = vec![0u8; (out_width * out_height * 4) as usize];

    for out_y in 0..out_height {
        for out_x in 0..out_width {
            let mut acc = [0.0_f32; 4];
            let mut samples = 0.0_f32;

            for dy in 0..factor {
                for dx in 0..factor {
                    let x = out_x * factor + dx;
                    let y = out_y * factor + dy;

                    if x >= width || y >= height {
                        continue;
                    }

                    let index = ((y * width + x) * 4) as usize;
                    let alpha = f32::from(rgba[index + 3]) / 255.0;

                    acc[0] += f32::from(rgba[index]) * alpha;
                    acc[1] += f32::from(rgba[index + 1]) * alpha;
                    acc[2] += f32::from(rgba[index + 2]) * alpha;
                    acc[3] += alpha;
                    samples += 1.0;
                }
            }

            if samples == 0.0 || acc[3] <= f32::EPSILON {
                continue;
            }

            let index = ((out_y * out_width + out_x) * 4) as usize;
            let alpha = acc[3] / samples;

            out[index] = (acc[0] / acc[3]).round().clamp(0.0, 255.0) as u8;
            out[index + 1] = (acc[1] / acc[3]).round().clamp(0.0, 255.0) as u8;
            out[index + 2] = (acc[2] / acc[3]).round().clamp(0.0, 255.0) as u8;
            out[index + 3] = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }

    (out_width, out_height, out)
}
