//! Prints the motion recipe for a cover image, or its raw CLIP embedding.
//!
//! cargo run -p cw-analysis --release --example recipe -- VISION_ONNX IMAGE [PCA_BIN] [--embedding] [--genre=NAME]

use cw_analysis::{ClipModel, GenreFamily, Models, Pca, ScenePrompts};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let want_embedding = args.iter().any(|a| a == "--embedding");
    let genre = args
        .iter()
        .find_map(|a| a.strip_prefix("--genre="))
        .map_or(GenreFamily::Unknown, GenreFamily::of_any);
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if pos.len() < 2 {
        eprintln!("usage: recipe VISION_ONNX IMAGE [PCA_BIN] [--embedding]");
        std::process::exit(2);
    }
    let read = |p: &str| std::fs::read(p).unwrap_or_else(|e| panic!("{p}: {e}"));
    let model = ClipModel::from_onnx_bytes(&read(pos[0])).expect("model");
    let pca = pos.get(2).map(|p| Pca::from_bytes(&read(p)).expect("pca"));
    let cover = cw_analysis::analyse(&read(pos[1])).expect("cover");
    let scenes = ScenePrompts::shipped();
    let models = Models {
        clip: Some(&model),
        scenes: &scenes,
        pca: pca.as_ref(),
        genre,
    };
    let (recipe, embedding) = cw_analysis::recipe(&cover, models).expect("recipe");
    if want_embedding {
        let e = embedding.unwrap_or_default();
        println!("{}", serde_json::to_string(&e).unwrap());
    } else {
        println!("{}", serde_json::to_string_pretty(&recipe).unwrap());
    }
}
