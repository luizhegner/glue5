//! Small CPU-only renderer for reviewing the real egui output in CI.
//! No game assets, window system or GPU required. Not used by the application.
use std::collections::HashMap;
use egui::{ColorImage, ImageData, Pos2, TextureId, epaint::{ClippedPrimitive, Primitive, textures::TexturesDelta}};

pub struct Raster {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
    textures: HashMap<TextureId, ColorImage>,
}

impl Raster {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, pixels: vec![0; width * height * 4], textures: HashMap::new() }
    }

    pub fn update(&mut self, delta: &TexturesDelta) {
        for (id, changes) in &delta.set {
          for change in changes {
            let ImageData::Color(image) = &change.image;
            if let Some([x, y]) = change.pos {
                let target = self.textures.get_mut(id).expect("texture exists before patch");
                for row in 0..image.size[1] {
                    let start = (y + row) * target.size[0] + x;
                    target.pixels[start..start + image.size[0]].copy_from_slice(&image.pixels[row * image.size[0]..(row + 1) * image.size[0]]);
                }
            } else {
                self.textures.insert(*id, (**image).clone());
            }
          }
        }
    }

    pub fn paint(&mut self, primitives: Vec<ClippedPrimitive>) {
        for pixel in self.pixels.chunks_exact_mut(4) { pixel.copy_from_slice(&[22, 24, 28, 255]); }
        for primitive in primitives {
            let Primitive::Mesh(mesh) = primitive.primitive else { continue };
            let texture = self.textures.get(&mesh.texture_id).expect("preview texture");
            for triangle in mesh.indices.chunks_exact(3) {
                let a = mesh.vertices[triangle[0] as usize];
                let b = mesh.vertices[triangle[1] as usize];
                let c = mesh.vertices[triangle[2] as usize];
                let edge = |a: Pos2, b: Pos2, p: Pos2| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
                let area = edge(a.pos, b.pos, c.pos);
                if area.abs() < 0.0001 { continue; }
                let clip = primitive.clip_rect;
                let left = a.pos.x.min(b.pos.x).min(c.pos.x).max(clip.left()).max(0.0).floor() as usize;
                let top = a.pos.y.min(b.pos.y).min(c.pos.y).max(clip.top()).max(0.0).floor() as usize;
                let right = a.pos.x.max(b.pos.x).max(c.pos.x).min(clip.right()).min(self.width as f32).ceil() as usize;
                let bottom = a.pos.y.max(b.pos.y).max(c.pos.y).min(clip.bottom()).min(self.height as f32).ceil() as usize;
                for y in top..bottom {
                    for x in left..right {
                        let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                        if !clip.contains(p) { continue; }
                        let weights = [edge(b.pos, c.pos, p) / area, edge(c.pos, a.pos, p) / area, edge(a.pos, b.pos, p) / area];
                        if weights.iter().any(|w| *w < 0.0) { continue; }
                        let uv = a.uv.to_vec2() * weights[0] + b.uv.to_vec2() * weights[1] + c.uv.to_vec2() * weights[2];
                        let tx = ((uv.x * texture.size[0] as f32) as usize).min(texture.size[0] - 1);
                        let ty = ((uv.y * texture.size[1] as f32) as usize).min(texture.size[1] - 1);
                        let texel = texture.pixels[ty * texture.size[0] + tx].to_array();
                        let colors = [a.color.to_array(), b.color.to_array(), c.color.to_array()];
                        let mut color = [0.0; 4];
                        for channel in 0..4 {
                            color[channel] = (0..3).map(|i| colors[i][channel] as f32 * weights[i]).sum::<f32>() * texel[channel] as f32 / 255.0;
                        }
                        let pixel = &mut self.pixels[(y * self.width + x) * 4..][..4];
                        for channel in 0..3 {
                            pixel[channel] = (color[channel] + pixel[channel] as f32 * (1.0 - color[3] / 255.0)).clamp(0.0, 255.0) as u8;
                        }
                    }
                }
            }
        }
    }

    pub fn save(&self, path: &std::path::Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = png::Encoder::new(file, self.width as u32, self.height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(&self.pixels).unwrap();
    }
}
