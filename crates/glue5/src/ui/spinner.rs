//! Tiny branded GTA V loading spinner used beside work-in-progress actions.
use egui::{Color32, ColorImage, Context, Pos2, Rect, TextureHandle, TextureOptions, Vec2, epaint::{Mesh, Shape, Vertex}};

const PNG: &[u8] = include_bytes!("../../assets/ui/spinner.png");

pub(super) fn load(ctx: &Context) -> TextureHandle {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(PNG));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().expect("embedded spinner PNG header");
    let mut bytes = vec![0; reader.output_buffer_size().expect("embedded spinner buffer size")];
    let info = reader.next_frame(&mut bytes).expect("embedded spinner PNG frame");
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
        png::ColorType::Indexed => panic!("embedded spinner PNG must be expanded"),
    };
    ctx.load_texture("gta-v-spinner", image, TextureOptions::LINEAR)
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
        mesh.vertices.push(Vertex { pos: center + rotated, uv, color: Color32::from_rgb(168, 216, 52) });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    painter.add(Shape::mesh(mesh));
}
