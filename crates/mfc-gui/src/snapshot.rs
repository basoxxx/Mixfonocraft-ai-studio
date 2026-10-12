//! Rasterizzatore software minimale per le mesh di egui: permette screenshot
//! dell'interfaccia senza GPU né finestra (test visivi, documentazione, CI).

use std::collections::HashMap;

use egui::epaint::{ClippedPrimitive, ImageData, Primitive, TextureId};
use egui::{Color32, ColorImage, Context, FullOutput, RawInput};

/// Immagine RGBA8 (non premoltiplicata) pronta per il salvataggio.
pub struct Screenshot {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Screenshot {
    /// Salva in PNG.
    pub fn save_png(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        let file = std::fs::File::create(path)?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(std::io::Error::other)?;
        w.write_image_data(&self.rgba).map_err(std::io::Error::other)?;
        Ok(())
    }
}

/// Renderer software con cache delle texture tra un frame e l'altro.
#[derive(Default)]
pub struct SoftwareRenderer {
    textures: HashMap<TextureId, ColorImage>,
}

impl SoftwareRenderer {
    /// Esegue `frames` passate di UI (per stabilizzare il layout) e rasterizza l'ultima.
    pub fn capture(
        &mut self,
        ctx: &Context,
        size: [u32; 2],
        pixels_per_point: f32,
        frames: usize,
        mut ui_fn: impl FnMut(&mut egui::Ui),
    ) -> Screenshot {
        let mut last = None;
        for _ in 0..frames.max(1) {
            let input = RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0] as f32, size[1] as f32) / pixels_per_point,
                )),
                ..Default::default()
            };
            ctx.set_pixels_per_point(pixels_per_point);
            let out = ctx.run_ui(input, &mut ui_fn);
            self.apply_textures(&out);
            last = Some(out);
        }
        let out: FullOutput = last.expect("almeno un frame");
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        let shot = self.rasterize(&prims, size, out.pixels_per_point);
        for id in &out.textures_delta.free {
            self.textures.remove(id);
        }
        shot
    }

    fn apply_textures(&mut self, out: &FullOutput) {
        for (id, deltas) in &out.textures_delta.set {
            for delta in deltas {
                let ImageData::Color(img) = &delta.image;
                match delta.pos {
                    None => {
                        self.textures.insert(*id, (**img).clone());
                    }
                    Some([x0, y0]) => {
                        if let Some(tex) = self.textures.get_mut(id) {
                            let [w, h] = img.size;
                            for y in 0..h {
                                for x in 0..w {
                                    let (tx, ty) = (x0 + x, y0 + y);
                                    if tx < tex.size[0] && ty < tex.size[1] {
                                        tex.pixels[ty * tex.size[0] + tx] = img.pixels[y * w + x];
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn rasterize(&self, prims: &[ClippedPrimitive], size: [u32; 2], ppp: f32) -> Screenshot {
        let (w, h) = (size[0] as usize, size[1] as usize);
        // Buffer premoltiplicato in float.
        let mut buf = vec![[0f32; 4]; w * h];
        for p in prims {
            let Primitive::Mesh(mesh) = &p.primitive else { continue };
            let Some(tex) = self.textures.get(&mesh.texture_id) else {
                continue;
            };
            let clip = p.clip_rect;
            let cx0 = ((clip.min.x * ppp).floor().max(0.0) as usize).min(w);
            let cy0 = ((clip.min.y * ppp).floor().max(0.0) as usize).min(h);
            let cx1 = ((clip.max.x * ppp).ceil().max(0.0) as usize).min(w);
            let cy1 = ((clip.max.y * ppp).ceil().max(0.0) as usize).min(h);
            for tri in mesh.indices.chunks_exact(3) {
                let v = [
                    &mesh.vertices[tri[0] as usize],
                    &mesh.vertices[tri[1] as usize],
                    &mesh.vertices[tri[2] as usize],
                ];
                let p: [(f32, f32); 3] = std::array::from_fn(|i| (v[i].pos.x * ppp, v[i].pos.y * ppp));
                let area = edge(p[0], p[1], p[2]);
                if area.abs() < 1e-8 {
                    continue;
                }
                let minx = p.iter().map(|q| q.0).fold(f32::MAX, f32::min).floor().max(cx0 as f32) as usize;
                let maxx = p.iter().map(|q| q.0).fold(f32::MIN, f32::max).ceil().min(cx1 as f32) as usize;
                let miny = p.iter().map(|q| q.1).fold(f32::MAX, f32::min).floor().max(cy0 as f32) as usize;
                let maxy = p.iter().map(|q| q.1).fold(f32::MIN, f32::max).ceil().min(cy1 as f32) as usize;
                let col: [[f32; 4]; 3] = std::array::from_fn(|i| {
                    let c = v[i].color;
                    [c.r() as f32, c.g() as f32, c.b() as f32, c.a() as f32].map(|x| x / 255.0)
                });
                for y in miny..maxy {
                    for x in minx..maxx {
                        let pt = (x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = edge(p[1], p[2], pt) / area;
                        let w1 = edge(p[2], p[0], pt) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                            continue;
                        }
                        let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                        let vv = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                        let t = sample(tex, u, vv);
                        let mut src = [0f32; 4];
                        for k in 0..4 {
                            src[k] = (w0 * col[0][k] + w1 * col[1][k] + w2 * col[2][k]) * t[k];
                        }
                        let d = &mut buf[y * w + x];
                        let ia = 1.0 - src[3];
                        for k in 0..4 {
                            d[k] = src[k] + d[k] * ia;
                        }
                    }
                }
            }
        }
        let mut rgba = Vec::with_capacity(w * h * 4);
        for px in buf {
            let a = px[3].clamp(0.0, 1.0);
            // Sfondo opaco: lo screenshot rappresenta la finestra.
            let un = |c: f32| ((c + (1.0 - a) * 0.0).clamp(0.0, 1.0) * 255.0).round() as u8;
            rgba.extend_from_slice(&[un(px[0]), un(px[1]), un(px[2]), 255]);
        }
        Screenshot {
            width: size[0],
            height: size[1],
            rgba,
        }
    }
}

fn edge(a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> f32 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

/// Campionamento bilineare (texel premoltiplicati).
fn sample(tex: &ColorImage, u: f32, v: f32) -> [f32; 4] {
    let [tw, th] = tex.size;
    let x = (u * tw as f32 - 0.5).clamp(0.0, (tw - 1) as f32);
    let y = (v * th as f32 - 0.5).clamp(0.0, (th - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(tw - 1), (y0 + 1).min(th - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let px = |x: usize, y: usize| -> [f32; 4] {
        let c: Color32 = tex.pixels[y * tw + x];
        [c.r() as f32, c.g() as f32, c.b() as f32, c.a() as f32].map(|x| x / 255.0)
    };
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bot = c[k] + (d[k] - c[k]) * fx;
        top + (bot - top) * fy
    })
}
