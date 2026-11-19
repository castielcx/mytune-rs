//! Real glass: a shape that shows what is behind it bent at its rim (and, for panels, frosted), like a lens.
//! egui has no such pass, so this is a paint callback with its own OpenGL program: it copies the frame as far
//! as it is drawn and draws it back displaced. Only the glow renderer runs it. Any other renderer (the headless
//! tests use wgpu) skips the callback, and the plain plate the caller draws over it is all that shows.

use eframe::egui::{self, Rect};
use eframe::glow::{self, HasContext as _};
use std::sync::{Arc, Mutex};

/// How a shape bends what is behind it. Lengths are in points.
#[derive(Clone, Copy)]
pub struct Lens {
    /// Corner radius of the shape.
    pub radius: f32,
    /// Width of the band along the rim that bends the picture.
    pub rim: f32,
    /// How far the rim pulls its picture in from outside (negative: pushes it out).
    pub bend: f32,
    /// Magnification of the middle (1 = none).
    pub zoom: f32,
    /// Blur radius of the picture behind (0 = clear glass).
    pub blur: f32,
    /// Colour split at the rim, as a share of `bend` (0 = none).
    pub split: f32,
}

/// A shape that draws what is behind `rect` back into it, bent as `lens` says. The caller paints its plate
/// (tint, rim) after it.
pub fn shape(rect: Rect, lens: Lens) -> egui::Shape {
    if !(rect.width() > 1.0 && rect.height() > 1.0) {
        return egui::Shape::Noop;
    }
    let callback = eframe::egui_glow::CallbackFn::new(move |info, painter| draw(painter.gl(), &info, lens));
    egui::Shape::Callback(egui::PaintCallback { rect, callback: Arc::new(callback) })
}

/// True once a lens was drawn in this run: the renderer runs it and the shaders built. Until then (and under
/// a renderer that skips it) what relies on the lens's blur to stay readable must stay solid.
pub fn works() -> bool {
    WORKS.load(std::sync::atomic::Ordering::Relaxed)
}

static WORKS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct Gl {
    /// The context these objects belong to (its address): another window's context gets its own set.
    ctx: usize,
    /// `None` when the shaders did not build here: the lens is then left out for good.
    made: Option<Made>,
}

#[derive(Clone, Copy)]
struct Made {
    program: glow::Program,
    vao: glow::VertexArray,
    tex: glow::Texture,
    u_rect: Option<glow::UniformLocation>,
    u_src: Option<glow::UniformLocation>,
    u_p: Option<glow::UniformLocation>,
    u_q: Option<glow::UniformLocation>,
}

static STATE: Mutex<Vec<Gl>> = Mutex::new(Vec::new());

const VERT: &str = r#"
void main() {
    vec2 p = vec2(float(gl_VertexID & 1), float(gl_VertexID >> 1));
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const FRAG: &str = r#"
#ifdef GL_ES
precision highp float;
#endif
uniform sampler2D u_tex;
uniform vec4 u_rect; // the shape in framebuffer pixels: x, y (from the bottom), width, height
uniform vec4 u_src;  // the copied part of the frame, same units
uniform vec4 u_p;    // radius, rim width, bend, zoom
uniform vec2 u_q;    // blur as a mip level, colour split
out vec4 o;

float box(vec2 p, vec2 b, float r) {
    vec2 q = abs(p) - b + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

vec3 take(vec2 at, float lod) {
    return textureLod(u_tex, (at - u_src.xy) / u_src.zw, lod).rgb;
}

void main() {
    vec2 half_size = u_rect.zw * 0.5;
    vec2 c = u_rect.xy + half_size;
    vec2 p = gl_FragCoord.xy - c;
    float r = min(u_p.x, min(half_size.x, half_size.y));
    float d = -box(p, half_size, r);
    float a = clamp(d + 0.5, 0.0, 1.0);
    if (a <= 0.0) {
        discard;
    }
    // outward direction of the nearest edge
    vec2 q = abs(p) - half_size + r;
    vec2 n = max(q.x, q.y) > 0.0 ? normalize(max(q, 0.0) + 1e-5) : (q.x > q.y ? vec2(1.0, 0.0) : vec2(0.0, 1.0));
    n *= vec2(p.x < 0.0 ? -1.0 : 1.0, p.y < 0.0 ? -1.0 : 1.0);
    float t = clamp(1.0 - d / max(u_p.y, 1.0), 0.0, 1.0);
    float k = t * t * u_p.z;
    vec2 at = c + p / u_p.w;
    vec3 col;
    if (u_q.x > 0.0) {
        float s = exp2(u_q.x) * 0.75;
        col = (take(at + n * k + vec2(s, s * 0.4), u_q.x) + take(at + n * k + vec2(-s * 0.4, s), u_q.x)
            + take(at + n * k + vec2(-s, -s * 0.4), u_q.x) + take(at + n * k + vec2(s * 0.4, -s), u_q.x)) * 0.25;
    } else {
        col = vec3(take(at + n * k * (1.0 + u_q.y), 0.0).r, take(at + n * k, 0.0).g, take(at + n * k * (1.0 - u_q.y), 0.0).b);
    }
    o = vec4(col, 1.0) * a;
}
"#;

fn make(gl: &glow::Context) -> Option<Made> {
    let version = eframe::egui_glow::ShaderVersion::get(gl);
    if !version.is_new_shader_interface() {
        return None; // GLSL 1.20 / ES 1.00: no gl_VertexID, no textureLod
    }
    unsafe {
        let program = gl.create_program().ok()?;
        for (kind, src) in [(glow::VERTEX_SHADER, VERT), (glow::FRAGMENT_SHADER, FRAG)] {
            let shader = gl.create_shader(kind).ok()?;
            gl.shader_source(shader, &format!("{}{src}", version.version_declaration()));
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                mytune_core::log::info(&format!("lens shader did not compile: {}", gl.get_shader_info_log(shader)));
                return None;
            }
            gl.attach_shader(program, shader);
            gl.delete_shader(shader); // freed with the program
        }
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            mytune_core::log::info(&format!("lens shader did not link: {}", gl.get_program_info_log(program)));
            return None;
        }
        let tex = gl.create_texture().ok()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        for (name, value) in [(glow::TEXTURE_MAG_FILTER, glow::LINEAR), (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE), (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE)] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
        }
        let u = |name: &str| gl.get_uniform_location(program, name);
        Some(Made { program, vao: gl.create_vertex_array().ok()?, tex, u_rect: u("u_rect"), u_src: u("u_src"), u_p: u("u_p"), u_q: u("u_q") })
    }
}

fn draw(gl: &Arc<glow::Context>, info: &egui::PaintCallbackInfo, lens: Lens) {
    let ctx = Arc::as_ptr(gl) as usize;
    let made = {
        let mut all = STATE.lock().unwrap_or_else(|e| e.into_inner());
        match all.iter().find(|g| g.ctx == ctx) {
            Some(g) => g.made,
            None => {
                let made = make(gl);
                all.push(Gl { ctx, made });
                made
            }
        }
    };
    let Some(m) = made else { return };
    let ppp = info.pixels_per_point;
    let [sw, sh] = info.screen_size_px.map(|v| v as f32);
    // the shape in framebuffer pixels, y from the bottom
    let r = info.viewport;
    let (x, y, w, h) = (r.left() * ppp, sh - r.bottom() * ppp, r.width() * ppp, r.height() * ppp);
    // copy the shape's surroundings too: the rim shows what lies just outside it
    let pad = (lens.bend.abs() * 1.3 + lens.blur * 2.0) * ppp + 2.0;
    let (x0, y0) = ((x - pad).floor().max(0.0), (y - pad).floor().max(0.0));
    let (x1, y1) = ((x + w + pad).ceil().min(sw), (y + h + pad).ceil().min(sh));
    if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
        return;
    }
    // a blur of `b` pixels reads the copy that many times smaller
    let lod = if lens.blur > 0.0 { (lens.blur * ppp).max(1.0).log2().max(0.5) } else { 0.0 };
    unsafe {
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(m.tex));
        gl.copy_tex_image_2d(glow::TEXTURE_2D, 0, glow::RGB, x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32, 0);
        if lod > 0.0 {
            gl.generate_mipmap(glow::TEXTURE_2D);
        }
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, if lod > 0.0 { glow::LINEAR_MIPMAP_LINEAR } else { glow::LINEAR } as i32);
        gl.use_program(Some(m.program));
        gl.uniform_4_f32(m.u_rect.as_ref(), x, y, w, h);
        gl.uniform_4_f32(m.u_src.as_ref(), x0, y0, x1 - x0, y1 - y0);
        gl.uniform_4_f32(m.u_p.as_ref(), lens.radius * ppp, lens.rim * ppp, lens.bend * ppp, lens.zoom.max(0.01));
        gl.uniform_2_f32(m.u_q.as_ref(), lod, lens.split);
        gl.bind_vertex_array(Some(m.vao));
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        gl.bind_vertex_array(None);
    }
    WORKS.store(true, std::sync::atomic::Ordering::Relaxed);
}
