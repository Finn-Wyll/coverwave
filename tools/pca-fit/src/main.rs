//! One-off tooling for the frozen parts of cover analysis.
//!
//! ```text
//! pca-fit prompts --text-model text_model.onnx --tokenizer tokenizer.json [--file assets/scene_prompts.json]
//!     Embeds the scene prompts with the CLIP text encoder and writes the
//!     embeddings back into the prompts file. Run after editing prompts (and
//!     bump its "version").
//!
//! pca-fit fit --vision-model vision_model.onnx [--out assets/pca.bin] INPUT...
//!     Fits the style-DNA PCA. INPUTs are image files, or folders searched
//!     recursively for images and for audio files with cover art (embedded
//!     or cover.jpg-style). Duplicate covers are counted once.
//!
//! pca-fit scenes --vision-model vision_model.onnx [--file assets/scene_prompts.json] IMAGE...
//!     Prints the top scene weights per image, to sanity-check prompts.
//! ```

use anyhow::{Context, Result, bail};
use cw_analysis::clip::{EMBED_DIM, normalize};
use cw_analysis::{ClipModel, Pca, ScenePrompts};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use tract_onnx::prelude::*;

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "gif"];
/// CLIP's text context length.
const CONTEXT: usize = 77;
/// Fewer covers than this make a noisy, unstable projection.
const RECOMMENDED_COVERS: usize = 500;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        bail!("usage: pca-fit prompts|fit|scenes ... (see the source header)");
    };
    let mut flags = BTreeMap::new();
    let mut inputs = Vec::new();
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        if let Some(name) = a.strip_prefix("--") {
            let v = it
                .next()
                .with_context(|| format!("--{name} needs a value"))?;
            flags.insert(name.to_string(), v.clone());
        } else {
            inputs.push(PathBuf::from(a));
        }
    }
    let flag = |n: &str| flags.get(n).with_context(|| format!("missing --{n}"));
    let prompts_file = flags
        .get("file")
        .cloned()
        .unwrap_or_else(|| "assets/scene_prompts.json".into());
    match cmd.as_str() {
        "prompts" => prompts(flag("text-model")?, flag("tokenizer")?, &prompts_file),
        "fit" => {
            let out = flags
                .get("out")
                .cloned()
                .unwrap_or_else(|| "assets/pca.bin".into());
            fit(flag("vision-model")?, &out, &inputs)
        }
        "scenes" => scenes(flag("vision-model")?, &prompts_file, &inputs),
        other => bail!("unknown command {other}"),
    }
}

fn prompts(model: &str, tokenizer: &str, file: &str) -> Result<()> {
    let tok = tokenizers::Tokenizer::from_file(tokenizer).map_err(anyhow::Error::msg)?;
    let text = tract_onnx::onnx()
        .model_for_path(model)?
        .with_input_fact(0, i64::fact([1, CONTEXT]).into())?
        .into_optimized()?
        .into_runnable()?;
    let embed = |s: &str| -> Result<Vec<f32>> {
        let enc = tok.encode(s, true).map_err(anyhow::Error::msg)?;
        let mut ids: Vec<i64> = enc.get_ids().iter().map(|&i| i as i64).collect();
        if ids.len() > CONTEXT {
            bail!("prompt too long: {s}");
        }
        ids.resize(CONTEXT, 0); // pad token "!" is id 0
        let t: Tensor = tract_ndarray::Array2::from_shape_vec((1, CONTEXT), ids)?.into();
        let out = text.run(tvec!(t.into()))?;
        Ok(normalize(
            out[0]
                .to_plain_array_view::<f32>()?
                .iter()
                .copied()
                .collect(),
        ))
    };

    let mut p = ScenePrompts::from_json(&std::fs::read_to_string(file)?)?;
    for scene in &mut p.scenes {
        let mut sum = vec![0.0f32; EMBED_DIM];
        for subject in &scene.subjects {
            for template in &p.templates {
                let e = embed(&template.replace("{}", subject))?;
                sum.iter_mut().zip(&e).for_each(|(s, x)| *s += x);
            }
        }
        scene.embedding = normalize(sum)
            .iter()
            .map(|x| (x * 1e6).round() / 1e6)
            .collect();
        println!(
            "{}: {} prompts",
            scene.name,
            scene.subjects.len() * p.templates.len()
        );
    }
    std::fs::write(file, compact_json(&p)?)?;
    println!("wrote {file}");
    Ok(())
}

/// Pretty JSON, but with each embedding on one line.
fn compact_json(p: &ScenePrompts) -> Result<String> {
    let pretty = serde_json::to_string_pretty(p)?;
    let mut out = String::new();
    let mut in_array = false;
    for line in pretty.lines() {
        let t = line.trim();
        if t.starts_with("\"embedding\": [") && !t.ends_with(']') && !t.ends_with("],") {
            in_array = true;
            out.push_str(line);
        } else if in_array {
            out.push_str(t);
            if t.starts_with(']') {
                in_array = false;
                out.push('\n');
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    Ok(out)
}

fn load_vision(path: &str) -> Result<ClipModel> {
    Ok(ClipModel::from_onnx_bytes(
        &std::fs::read(path).with_context(|| path.to_string())?,
    )?)
}

fn fit(model: &str, out: &str, inputs: &[PathBuf]) -> Result<()> {
    let clip = load_vision(model)?;
    let covers = collect_covers(inputs);
    println!("{} distinct covers", covers.len());
    let mut embeddings = Vec::new();
    for (i, (name, bytes)) in covers.iter().enumerate() {
        match cw_analysis::analyse(bytes) {
            Ok(cover) => embeddings.push(clip.embed(&cover.image)?),
            Err(e) => eprintln!("skip {name}: {e}"),
        }
        if (i + 1) % 50 == 0 {
            println!("  embedded {}/{}", i + 1, covers.len());
        }
    }
    let pca = Pca::fit(&embeddings)?;
    std::fs::write(out, pca.to_bytes())?;
    let total: f32 = pca.std.iter().map(|s| s * s).sum();
    println!(
        "wrote {out}: {} covers, version {}, component std {:?} (sum of variance {total:.4})",
        pca.samples,
        pca.version(),
        pca.std
            .iter()
            .map(|s| (s * 1000.0).round() / 1000.0)
            .collect::<Vec<_>>()
    );
    if embeddings.len() < RECOMMENDED_COVERS {
        println!(
            "warning: {} covers is a small corpus; {RECOMMENDED_COVERS}+ gives a steadier projection",
            embeddings.len()
        );
    }
    Ok(())
}

fn scenes(model: &str, file: &str, images: &[PathBuf]) -> Result<()> {
    let clip = load_vision(model)?;
    let prompts = ScenePrompts::from_json(&std::fs::read_to_string(file)?)?;
    if !prompts.is_ready(EMBED_DIM) {
        bail!("{file} has no embeddings yet; run `pca-fit prompts` first");
    }
    for path in images {
        let cover = cw_analysis::analyse(&std::fs::read(path)?)?;
        let w = prompts.weights(&clip.embed(&cover.image)?);
        let mut top: Vec<(&String, &f32)> = w.iter().collect();
        top.sort_by(|a, b| b.1.total_cmp(a.1));
        let shown: Vec<String> = top
            .iter()
            .take(3)
            .map(|(k, v)| format!("{k} {v:.2}"))
            .collect();
        println!("{}: {}", path.display(), shown.join(", "));
    }
    Ok(())
}

/// Every distinct cover reachable from `inputs`, as (label, bytes).
fn collect_covers(inputs: &[PathBuf]) -> Vec<(String, Vec<u8>)> {
    fn is_image(p: &Path) -> bool {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| IMAGE_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
    }
    fn walk(p: &Path, out: &mut Vec<PathBuf>) {
        if p.is_dir() {
            let Ok(rd) = std::fs::read_dir(p) else { return };
            let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            entries.sort();
            for e in entries {
                walk(&e, out);
            }
        } else {
            out.push(p.to_path_buf());
        }
    }
    let mut files = Vec::new();
    for i in inputs {
        walk(i, &mut files);
    }
    let mut seen = HashSet::new();
    let mut covers = Vec::new();
    for f in files {
        let bytes = if is_image(&f) {
            std::fs::read(&f).ok()
        } else if cw_audio::is_audio_file(&f) {
            cw_audio::cover_art(&f)
        } else {
            None
        };
        if let Some(b) = bytes
            && seen.insert(Sha256::digest(&b))
        {
            covers.push((f.display().to_string(), b));
        }
    }
    covers
}
