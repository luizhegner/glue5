//! The build's loading spinner; a code-drawn ring is used until it is available.
use egui::{Color32, ColorImage, Context, Pos2, Rect, TextureHandle, TextureOptions, Vec2, epaint::{Mesh, Shape, Vertex}};

pub(super) fn load(ctx: &Context) -> TextureHandle {
    ctx.load_texture("loading-spinner", fallback(), TextureOptions::LINEAR)
}

fn fallback() -> ColorImage {
    let mut rgba = Vec::with_capacity(64*64*4);
    for y in 0..64 { for x in 0..64 {
        let dx = x as f32-31.5;
        let dy = y as f32-31.5;
        let distance = dx.hypot(dy);
        let alpha = if (19.0..25.0).contains(&distance) {
            let phase = (dy.atan2(dx)+std::f32::consts::PI)/std::f32::consts::TAU;
            (40.0+215.0*phase) as u8
        } else { 0 };
        rgba.extend_from_slice(&[255,255,255,alpha]);
    } }
    ColorImage::from_rgba_unmultiplied([64,64],&rgba)
}

fn decode(bytes:&[u8]) -> Option<ColorImage> {
    if bytes.len()>256*1024 { return None; }
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let (w,h) = (reader.info().width,reader.info().height);
    if w==0 || h==0 || w>256 || h>256 { return None; }
    let len = reader.output_buffer_size()?;
    if len>256*256*4 { return None; }
    let mut bytes = vec![0;len];
    let info = reader.next_frame(&mut bytes).ok()?;
    let data = &bytes[..info.buffer_size()];
    let size = [info.width as usize, info.height as usize];
    let image = match info.color_type {
        png::ColorType::Rgba => ColorImage::from_rgba_unmultiplied(size, data),
        png::ColorType::Rgb => ColorImage::from_rgb(size, data),
        png::ColorType::GrayscaleAlpha => {
            let rgba: Vec<_> = data.chunks_exact(2).flat_map(|px| [px[0], px[0], px[0], px[1]]).collect();
            ColorImage::from_rgba_unmultiplied(size, &rgba)
        }
        png::ColorType::Grayscale => {
            let rgb: Vec<_> = data.iter().flat_map(|&gray| [gray, gray, gray]).collect();
            ColorImage::from_rgb(size, &rgb)
        }
        png::ColorType::Indexed => return None,
    };
    Some(image)
}

pub(super) fn update(ctx:&Context,texture:&mut TextureHandle,game:Option<&std::path::Path>,build:&str) {
    let path = game.map(|root| root.join("b").join(build).join("title/spinner.png"));
    let id = egui::Id::new("spinner-game-path");
    if ctx.data(|d| d.get_temp::<Option<std::path::PathBuf>>(id)).as_ref()==Some(&path) { return; }
    let image = path.as_deref().and_then(|path| {
        if std::fs::metadata(path).ok()?.len()>256*1024 { return None; }
        decode(&std::fs::read(path).ok()?)
    }).unwrap_or_else(fallback);
    texture.set(image,TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id,path));
}

/// Draw the spinner as a rotated textured quad. The original 64 px art stays
/// crisp and gets the shell's lime accent without replacing its GTA V shape.
pub(super) fn paint(painter: &egui::Painter, texture: &TextureHandle, rect: Rect, angle: f32) {
    let center = rect.center();
    let half = rect.size() * 0.5;
    let (sin, cos) = angle.sin_cos();
    let corners = [
        (Vec2::new(-half.x, -half.y), Pos2::new(0.0, 0.0)),
        (Vec2::new(half.x, -half.y), Pos2::new(1.0, 0.0)),
        (Vec2::new(half.x, half.y), Pos2::new(1.0, 1.0)),
        (Vec2::new(-half.x, half.y), Pos2::new(0.0, 1.0)),
    ];
    let mut mesh = Mesh::with_texture(texture.id());
    for (offset, uv) in corners {
        let rotated = Vec2::new(offset.x * cos - offset.y * sin, offset.x * sin + offset.y * cos);
        mesh.vertices.push(Vertex { pos: center + rotated, uv, color: Color32::WHITE });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    painter.add(Shape::mesh(mesh));
}
