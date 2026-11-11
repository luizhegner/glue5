//! The WebAssembly build's loading artwork fills the actual output.
//! Source coordinates are stored as fractions, not a fixed-size viewport.
//! Images are read from the selected build, never bundled.
use std::collections::HashMap;
use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use egui::{Color32, ColorImage, Context, Painter, Pos2, Rect, TextureHandle, TextureOptions, Vec2, pos2, vec2};
use egui::epaint::{Mesh, Vertex};

const FILE_LIMIT: u64 = 2 * 1024 * 1024;
const LAYER_LIMIT: usize = 8 * 1024 * 1024;
const SCENE_LIMIT: usize = 16 * 1024 * 1024;
const CAMERA_HEIGHTS: f32 = 1229.0 / 720.0;
const TURN: f64 = 14.0;
const SLIDE: f64 = 0.85;
const TWEEN: f64 = 42.0;

#[derive(Clone, Copy)]
struct Spec {
    file: &'static str,
    size: [f32; 2],
    at: [f32; 2],
    to_x: f32,
    yr: [f32; 2],
    to_xr: f32,
    background: bool,
}

const fn bg(file: &'static str, yr: f32, to_yr: f32, to_xr: f32) -> Spec {
    Spec { file, size: [2.0, 1.0], at: [0.0, 0.0], to_x: 0.0, yr: [yr, to_yr], to_xr, background:true }
}
const fn fg(file: &'static str, size: [f32; 2], at: [f32; 2], to_x: f32, yr: f32) -> Spec {
    Spec { file, size: [size[0]/720.0,size[1]/720.0], at:[at[0]/1280.0,at[1]/720.0],
        to_x:to_x/1280.0, yr:[yr,yr], to_xr:0.0, background:false }
}

const SCENES: &[&[Spec]] = &[
    &[bg("beach_bg.webp", 20.0, -6.0, 5.0), fg("beach_fg.webp", [343.0,629.0], [480.0,95.0], 720.0, 0.0)],
    &[bg("ls0_background.webp",7.0,-7.0,0.0), fg("ls0_foreground.webp",[299.0,599.0],[360.0,140.0],560.0,7.0)],
    &[bg("ls1_background.webp",6.0,-12.0,0.0), fg("ls1_foreground.webp",[503.0,206.0],[300.0,465.0],480.0,0.0), fg("ls1_foreground_franklin.webp",[368.0,588.0],[440.0,140.0],745.0,0.0)],
    &[bg("ls2_background.webp",7.0,-5.0,5.0), fg("ls2_foreground.webp",[311.0,558.0],[745.0,200.0],450.0,0.0), fg("ls2_foreground_chop.webp",[320.0,512.0],[640.0,260.0],310.0,0.0)],
    &[bg("ls3_background.webp",12.0,-4.0,0.0), fg("ls3_foreground.webp",[412.0,609.0],[440.0,120.0],730.0,0.0)],
    &[bg("ls4_background.webp",7.0,-4.0,5.0), fg("ls4_foreground.webp",[251.0,643.0],[400.0,85.0],250.0,0.0)],
    &[bg("ls5_background.webp",7.0,-6.0,0.0), fg("ls5_foreground.webp",[389.0,640.0],[450.0,95.0],700.0,0.0)],
    &[bg("ls6_background.webp",7.0,0.0,5.0), fg("ls6_foreground.webp",[367.0,603.0],[410.0,115.0],800.0,0.0)],
    &[bg("ls7_background.webp",6.0,-6.0,5.0), fg("ls7_foreground.webp",[311.0,625.0],[860.0,95.0],520.0,0.0)],
    &[bg("ls8_background.webp",7.0,-5.0,5.0), fg("ls8_foreground.webp",[492.0,497.0],[400.0,190.0],660.0,0.0)],
    &[bg("ls9_background.webp",7.0,-3.0,5.0), fg("ls9_foreground.webp",[331.0,675.0],[220.0,50.0],140.0,0.0)],
    &[bg("ls10_background.webp",6.0,-5.0,5.0), fg("ls10_foreground.webp",[352.0,682.0],[545.0,65.0],485.0,0.0)],
    &[bg("ls11_background.webp",7.0,-3.0,5.0), fg("ls11_foreground.webp",[344.0,669.0],[309.0,51.0],424.0,0.0)],
    &[bg("ls12_background.webp",6.0,-4.0,0.0), fg("ls12_foreground_michael.webp",[462.0,677.0],[345.0,42.0],220.0,0.0)],
    &[bg("ls13_background.webp",5.0,-6.0,5.0), fg("ls13_foreground.webp",[399.0,681.0],[430.0,39.0],250.0,0.0)],
    &[bg("ls14_background.webp",6.0,-2.0,5.0), fg("ls14_foreground.webp",[392.0,631.0],[650.0,95.0],850.0,0.0)],
    &[bg("ls15_background.webp",6.0,-3.0,5.0), fg("ls15_foreground.webp",[385.0,642.0],[404.0,78.0],261.0,0.0)],
    &[bg("ls16_background.webp",6.0,-3.0,5.0), fg("ls16_foreground.webp",[300.0,600.0],[300.0,150.0],400.0,0.0)],
];

fn project(spec: Spec, uv: Pos2, progress: f32, stage: Rect, slide: f32) -> Pos2 {
    let q = progress.clamp(0.0,1.0);
    let yr = (spec.yr[0] + (spec.yr[1] - spec.yr[0]) * q).to_radians();
    let xr = (spec.to_xr * q).to_radians();
    let (sy,cy) = yr.sin_cos();
    let (sx,cx) = xr.sin_cos();
    if spec.background {
        // Keep the existing full-output cover behavior: tilting the backing
        // plane can expose black corners on the phone's wider aspect ratio.
        let h = stage.height().max(stage.width()/2.0)*1.08;
        let size = vec2(h*2.0,h);
        let drift = yr.sin()*stage.width()*0.06;
        return stage.min+(stage.size()-size)*0.5+vec2(uv.x*size.x+drift+slide,uv.y*size.y);
    }
    let size = vec2(spec.size[0],spec.size[1])*stage.height();
    let at = vec2((spec.at[0]+(spec.to_x-spec.at[0])*q)*stage.width(),spec.at[1]*stage.height());
    let u = uv.x * size.x;
    let v = uv.y * size.y;
    let x = at.x + u * cy + v * sx * sy;
    let y = at.y + v * cx;
    let z = u * sy - v * sx * cy;
    let camera = stage.height()*CAMERA_HEIGHTS;
    let perspective = camera / (camera - z).max(1.0);
    let center = stage.size()*0.5;
    stage.min + center + (vec2(x,y)-center)*perspective + vec2(slide,0.0)
}

fn mesh(texture: &TextureHandle, spec: Spec, progress: f32, stage: Rect, slide: f32) -> Mesh {
    // Subdivide perspective quads: egui interpolates UVs affinely. A small grid
    // approximates the original camera without introducing another GPU pass.
    const GRID: usize = 8;
    let mut mesh = Mesh::with_texture(texture.id());
    for y in 0..=GRID {
        for x in 0..=GRID {
            let uv = pos2(x as f32 / GRID as f32, y as f32 / GRID as f32);
            mesh.vertices.push(Vertex { pos: project(spec,uv,progress,stage,slide), uv, color:Color32::WHITE });
        }
    }
    for y in 0..GRID {
        for x in 0..GRID {
            let a = (y * (GRID + 1) + x) as u32;
            let b = a + 1;
            let c = a + (GRID + 1) as u32;
            mesh.indices.extend_from_slice(&[a,b,c,b,c+1,c]);
        }
    }
    mesh
}

fn decode(bytes: &[u8]) -> Option<ColorImage> {
    if bytes.len() as u64 > FILE_LIMIT { return None; }
    let mut decoder = image_webp::WebPDecoder::new(BufReader::new(Cursor::new(bytes))).ok()?;
    decoder.set_memory_limit(LAYER_LIMIT);
    let (w,h) = decoder.dimensions();
    if w == 0 || h == 0 || w > 2048 || h > 2048 { return None; }
    let budget = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
    if budget > LAYER_LIMIT { return None; }
    let len = decoder.output_buffer_size()?;
    if len > LAYER_LIMIT { return None; }
    let mut pixels = vec![0;len];
    decoder.read_image(&mut pixels).ok()?;
    let size = [w as usize,h as usize];
    Some(if decoder.has_alpha() { ColorImage::from_rgba_unmultiplied(size,&pixels) }
        else { ColorImage::from_rgb(size,&pixels) })
}

fn read_scene(root: &Path, index: usize) -> Option<Vec<ColorImage>> {
    let root = root.canonicalize().ok()?;
    let mut total = 0usize;
    SCENES.get(index)?.iter().map(|spec| {
        let path = root.join("art").join(spec.file).canonicalize().ok()?;
        if !path.starts_with(&root) || std::fs::metadata(&path).ok()?.len() > FILE_LIMIT { return None; }
        let image = decode(&std::fs::read(path).ok()?)?;
        total = total.checked_add(image.pixels.len().checked_mul(4)?)?;
        (total <= SCENE_LIMIT).then_some(image)
    }).collect()
}

struct Loaded { generation:u64, index:usize, images:Option<Vec<ColorImage>> }

pub(super) struct Title {
    root:Option<PathBuf>,
    generation:u64,
    available:Vec<usize>,
    textures:HashMap<usize,Option<Vec<TextureHandle>>>,
    tx:Sender<Loaded>,
    rx:Receiver<Loaded>,
    started:Instant,
    shown:Option<usize>,
    previous:Option<usize>,
    switched:Instant,
}

impl Title {
    pub(super) fn new() -> Self {
        let (tx,rx) = channel();
        Self { root:None, generation:0, available:Vec::new(), textures:HashMap::new(), tx,rx,
            started:Instant::now(),shown:None,previous:None,switched:Instant::now() }
    }

    pub(super) fn release(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.root = None;
        self.available.clear();
        self.textures.clear();
        self.shown = None;
        self.previous = None;
        self.discard_pending();
    }

    pub(super) fn discard_pending(&mut self) { while self.rx.try_recv().is_ok() {} }

    fn locate(&mut self, game:Option<&Path>, build:&str) {
        let root = game.map(|g| g.join("b").join(build).join("title"));
        if self.root == root { return; }
        self.release();
        self.root = root;
        if let Some(root) = &self.root {
            self.available = SCENES.iter().enumerate()
                .filter(|(_,layers)| layers.iter().all(|l| root.join("art").join(l.file).is_file()))
                .map(|(index,_)| index).collect();
        }
        self.started = Instant::now();
    }

    fn ensure(&mut self, ctx:&Context, index:usize) {
        if self.textures.contains_key(&index) { return; }
        let Some(root) = self.root.clone() else { return; };
        self.textures.insert(index,None);
        let (tx,generation,ctx) = (self.tx.clone(),self.generation,ctx.clone());
        if let Err(error) = std::thread::Builder::new().name("game-title-decode".into()).spawn(move || {
            let images = read_scene(&root,index);
            let _ = tx.send(Loaded {generation,index,images});
            ctx.request_repaint();
        }) { log::warn!("title: cannot decode scene: {error}"); }
    }

    pub(super) fn draw(&mut self,ctx:&Context,painter:&Painter,output:Rect,game:Option<&Path>,build:&str,motion:bool) -> bool {
        self.locate(game,build);
        while let Ok(loaded) = self.rx.try_recv() {
            if loaded.generation != self.generation || !self.textures.contains_key(&loaded.index) { continue; }
            if let Some(images) = loaded.images {
                let textures = images.into_iter().enumerate().map(|(layer,image)| {
                    ctx.load_texture(format!("game-title-{}-{layer}",loaded.index),image,TextureOptions::LINEAR)
                }).collect();
                self.textures.insert(loaded.index,Some(textures));
            }
        }
        if self.available.is_empty() { return false; }
        let elapsed = self.started.elapsed().as_secs_f64();
        let turn = if motion { (elapsed / TURN) as usize } else { 0 };
        let wanted = self.available[turn % self.available.len()];
        self.ensure(ctx,wanted);
        if self.shown != Some(wanted) && self.textures.get(&wanted).is_some_and(Option::is_some) {
            self.previous = self.shown;
            self.shown = Some(wanted);
            self.switched = Instant::now();
        }
        let Some(shown) = self.shown else { return false; };
        let stage = output;
        let painter = painter.with_clip_rect(stage.intersect(output));
        let since = self.switched.elapsed().as_secs_f64();
        let p = if motion { (since / SLIDE).clamp(0.0,1.0) as f32 } else { 1.0 };
        let ease = if p < 0.5 { 4.0*p*p*p } else { 1.0-(-2.0*p+2.0).powi(3)*0.5 };
        let side = if turn % 2 == 0 { 1.0 } else { -1.0 };
        let progress = if motion { ((since-SLIDE).max(0.0)/TWEEN).min(1.0) as f32 } else { 0.0 };
        if p < 1.0 {
            if let Some(previous) = self.previous { self.paint(&painter,previous,stage,1.0/3.0,-side*output.width()*ease); }
        } else { self.previous = None; }
        self.paint(&painter,shown,stage,progress,side*output.width()*(1.0-ease));
        let next = self.available[(turn+1) % self.available.len()];
        if p >= 1.0 { self.ensure(ctx,next); }
        self.textures.retain(|index,_| *index == wanted || *index == shown || Some(*index) == self.previous || *index == next);
        if motion { ctx.request_repaint_after(Duration::from_millis(33)); }
        true
    }

    fn paint(&self,painter:&Painter,index:usize,stage:Rect,progress:f32,slide:f32) {
        let Some(Some(textures)) = self.textures.get(&index) else { return; };
        for (texture,spec) in textures.iter().zip(SCENES[index]) {
            painter.add(mesh(texture,*spec,progress,stage,slide));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_scales_to_real_output_and_preserves_foreground_aspect() {
        let stage = Rect::from_min_size(Pos2::ZERO,vec2(1000.0,500.0));
        let spec = SCENES[0][1];
        let start = project(spec,Pos2::ZERO,0.0,stage,0.0);
        let end = project(spec,Pos2::ZERO,1.0,stage,0.0);
        assert!((start.x-375.0).abs()<0.001);
        assert!((end.x-562.5).abs()<0.001);
        let corner = project(spec,pos2(1.0,1.0),0.0,stage,0.0);
        assert!(((corner.x-start.x)/(corner.y-start.y)-343.0/629.0).abs()<0.001);
        for scene in SCENES { for spec in *scene { for q in [0.0,0.5,1.0] {
            let p = project(*spec,pos2(1.0,1.0),q,stage,0.0);
            assert!(p.x.is_finite() && p.y.is_finite());
        } } }
    }

    #[test]
    fn backing_images_cover_the_real_output_through_every_tween() {
        for size in [vec2(2340.0,1080.0),vec2(844.0,390.0),vec2(960.0,432.0)] {
            let output=Rect::from_min_size(pos2(35.0,11.0),size);
            for scene in SCENES { for q in [0.0,0.5,1.0] {
                let a=project(scene[0],Pos2::ZERO,q,output,0.0);
                let b=project(scene[0],pos2(1.0,1.0),q,output,0.0);
                assert!(Rect::from_min_max(a,b).contains_rect(output));
            } }
        }
    }

    #[test]
    fn webp_decode_keeps_foreground_alpha_and_rejects_invalid_input() {
        let rgba = [255,0,0,255,0,255,0,0];
        let mut bytes = Vec::new();
        image_webp::WebPEncoder::new(&mut bytes).encode(&rgba,2,1,image_webp::ColorType::Rgba8).unwrap();
        let image = decode(&bytes).unwrap();
        assert_eq!(image.size,[2,1]);
        assert_eq!(image.pixels[0],Color32::RED);
        assert_eq!(image.pixels[1].a(),0);
        assert!(decode(b"RIFF").is_none());
        assert!(decode(&vec![0;FILE_LIMIT as usize+1]).is_none());
    }
}
