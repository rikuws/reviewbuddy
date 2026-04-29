use gpui::*;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OverviewShaderVariant {
    Flow,
    Bands,
    Ember,
    Lagoon,
    Aurora,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ShaderCornerMask {
    pub top_left: bool,
    pub top_right: bool,
    pub bottom_right: bool,
    pub bottom_left: bool,
}

impl ShaderCornerMask {
    pub const LEFT: Self = Self {
        top_left: true,
        top_right: false,
        bottom_right: false,
        bottom_left: true,
    };

    pub const TOP: Self = Self {
        top_left: true,
        top_right: true,
        bottom_right: false,
        bottom_left: false,
    };

    pub const ALL: Self = Self {
        top_left: true,
        top_right: true,
        bottom_right: true,
        bottom_left: true,
    };

    const fn any(self) -> bool {
        self.top_left || self.top_right || self.bottom_right || self.bottom_left
    }
}

pub const OVERVIEW_SHADER_GLSL: &str = r#"
void mainImage(out vec4 fragColor, vec2 fragCoord) {
    float mr = min(iResolution.x, iResolution.y);
    vec2 uv = (fragCoord * 2.0 - iResolution.xy) / mr;

    float d = -iTime * 0.5;
    float a = 0.0;
    for (float i = 0.0; i < 8.0; ++i) {
        a += cos(i - d - a * uv.x);
        d += sin(uv.y * i + a);
    }
    d += iTime * 0.5;
    vec3 col = vec3(cos(uv * vec2(d, a)) * 0.6 + 0.4, cos(a + d) * 0.5 + 0.5);
    col = cos(col * cos(vec3(d, a, 2.5)) * 0.5 + 0.5);
    fragColor = vec4(col, 1);
}
"#;

pub fn opengl_shader_surface(seed: impl Into<String>) -> Div {
    opengl_shader_surface_variant(seed, OverviewShaderVariant::Flow)
}

pub fn opengl_shader_surface_variant(
    seed: impl Into<String>,
    variant: OverviewShaderVariant,
) -> Div {
    platform::shader_surface(seed.into(), variant)
}

pub fn opengl_shader_surface_with_corner_mask(
    seed: impl Into<String>,
    radius: Pixels,
    mask_color: Rgba,
    corners: ShaderCornerMask,
) -> Div {
    opengl_shader_surface_variant_with_corner_mask(
        seed,
        OverviewShaderVariant::Flow,
        radius,
        mask_color,
        corners,
    )
}

pub fn opengl_shader_surface_variant_with_corner_mask(
    seed: impl Into<String>,
    variant: OverviewShaderVariant,
    radius: Pixels,
    mask_color: Rgba,
    corners: ShaderCornerMask,
) -> Div {
    // GPUI's overflow mask is rectangular, so the CVPixelBuffer still needs
    // the painted corner mask. Rounding the wrapper separately prevents its
    // fallback/background layer from showing through as square corners.
    let mut surface = platform::shader_surface(seed.into(), variant).rounded(radius);
    if corners.any() {
        surface = surface.child(shader_corner_mask(radius, mask_color, corners));
    }
    surface
}

fn shader_corner_mask(
    radius: Pixels,
    mask_color: Rgba,
    corners: ShaderCornerMask,
) -> impl IntoElement {
    canvas(
        move |_, _, _| (),
        move |bounds, _, window, _| {
            paint_shader_corner_mask(window, bounds, radius, mask_color, corners);
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

fn paint_shader_corner_mask(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    radius: Pixels,
    color: Rgba,
    corners: ShaderCornerMask,
) {
    let radius = f32::from(radius)
        .min(f32::from(bounds.size.width) / 2.0)
        .min(f32::from(bounds.size.height) / 2.0);
    if radius <= 0.0 {
        return;
    }

    let radius = px(radius);
    let control = px(f32::from(radius) * 0.552_284_8);
    let left = bounds.left();
    let right = bounds.right();
    let top = bounds.top();
    let bottom = bounds.bottom();

    if corners.top_left {
        let mut builder = PathBuilder::fill();
        builder.move_to(point(left, top));
        builder.line_to(point(left + radius, top));
        builder.cubic_bezier_to(
            point(left, top + radius),
            point(left + radius - control, top),
            point(left, top + radius - control),
        );
        builder.line_to(point(left, top));
        builder.close();
        paint_mask_path(window, builder, color);
    }

    if corners.top_right {
        let mut builder = PathBuilder::fill();
        builder.move_to(point(right, top));
        builder.line_to(point(right - radius, top));
        builder.cubic_bezier_to(
            point(right, top + radius),
            point(right - radius + control, top),
            point(right, top + radius - control),
        );
        builder.line_to(point(right, top));
        builder.close();
        paint_mask_path(window, builder, color);
    }

    if corners.bottom_right {
        let mut builder = PathBuilder::fill();
        builder.move_to(point(right, bottom));
        builder.line_to(point(right, bottom - radius));
        builder.cubic_bezier_to(
            point(right - radius, bottom),
            point(right, bottom - radius + control),
            point(right - radius + control, bottom),
        );
        builder.line_to(point(right, bottom));
        builder.close();
        paint_mask_path(window, builder, color);
    }

    if corners.bottom_left {
        let mut builder = PathBuilder::fill();
        builder.move_to(point(left, bottom));
        builder.line_to(point(left, bottom - radius));
        builder.cubic_bezier_to(
            point(left + radius, bottom),
            point(left, bottom - radius + control),
            point(left + radius - control, bottom),
        );
        builder.line_to(point(left, bottom));
        builder.close();
        paint_mask_path(window, builder, color);
    }
}

fn paint_mask_path(window: &mut Window, builder: PathBuilder, color: Rgba) {
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::OverviewShaderVariant;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::OnceLock;
    use std::time::Instant;

    use core_foundation::base::{CFType, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    use core_video::pixel_buffer::{
        kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, CVPixelBuffer, CVPixelBufferKeys,
    };
    use gpui::prelude::*;
    use gpui::*;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
    use objc2_core_image::{
        kCIContextUseSoftwareRenderer, CIColorKernel, CIContext, CIContextOption, CIImage, CIVector,
    };
    use objc2_core_video::CVPixelBuffer as ObjcCVPixelBuffer;
    use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

    const TARGET_FRAME_RATE: f32 = 30.0;

    const FLOW_CORE_IMAGE_SHADER: &str = r#"
kernel vec4 overviewShader(float iTime, vec2 iResolution) {
    vec2 fragCoord = destCoord();
    float mr = min(iResolution.x, iResolution.y);
    vec2 uv = (fragCoord * 2.0 - iResolution) / mr;

    float d = -iTime * 0.5;
    float a = 0.0;
    for (float i = 0.0; i < 8.0; ++i) {
        a += cos(i - d - a * uv.x);
        d += sin(uv.y * i + a);
    }
    d += iTime * 0.5;
    vec3 col = vec3(cos(uv * vec2(d, a)) * 0.6 + 0.4, cos(a + d) * 0.5 + 0.5);
    col = cos(col * cos(vec3(d, a, 2.5)) * 0.5 + 0.5);
    return vec4(col, 1.0);
}
"#;

    const BANDS_CORE_IMAGE_SHADER: &str = r#"
kernel vec4 overviewShader(float iTime, vec2 iResolution) {
    vec2 fragCoord = destCoord();
    vec2 uv = fragCoord / iResolution;

    float d = -(iTime * 0.3);
    float a = 0.0;

    for (float i = 0.0; i < 9.0; ++i) {
        a += cos(d + i * uv.x - a);
        d += 0.5 * sin(a + i * uv.y);
    }

    d += iTime * 0.3;

    float r = cos(uv.x * a) * 0.7 + 0.3;
    float g = cos(uv.y * d) * 0.5 + 0.2;
    float b = cos(a + d) * 0.3 + 0.5;
    vec3 col = vec3(r, g, b);
    col = cos(col * cos(vec3(d, a, 2.5)) * 0.5 + 0.5);

    return vec4(col, 1.0);
}
"#;

    const EMBER_CORE_IMAGE_SHADER: &str = r#"
kernel vec4 overviewShader(float iTime, vec2 iResolution) {
    vec2 fragCoord = destCoord();
    vec2 uv = fragCoord / iResolution;

    float t = iTime * 0.14;
    float d = -t;
    float a = 0.0;

    for (float i = 0.0; i < 9.0; ++i) {
        a += cos(d + i * uv.x - a);
        d += 0.5 * sin(a + i * uv.y);
    }

    d += t;

    float r = cos(uv.x * a) * 0.7 + 0.3;
    float g = cos(uv.y * d) * 0.5 + 0.2;
    float b = cos(a + d) * 0.3 + 0.5;
    vec3 col = vec3(r, g, b);
    col = cos(col * cos(vec3(d, a, 2.5)) * 0.5 + 0.5);
    col = vec3(
        col.r * 0.68 + col.g * 0.12 + 0.16,
        col.g * 0.58 + col.b * 0.14 + 0.08,
        col.b * 0.62 + col.r * 0.10 + 0.10
    );
    col = min(max(col, vec3(0.0)), vec3(1.0));

    return vec4(col, 1.0);
}
"#;

    const LAGOON_CORE_IMAGE_SHADER: &str = r#"
kernel vec4 overviewShader(float iTime, vec2 iResolution) {
    vec2 fragCoord = destCoord();
    vec2 uv = fragCoord / iResolution;

    float t = iTime * 0.14;
    float d = -t;
    float a = 0.0;

    for (float i = 0.0; i < 9.0; ++i) {
        a += cos(d + i * uv.x - a);
        d += 0.5 * sin(a + i * uv.y);
    }

    d += t;

    float r = cos(uv.x * a) * 0.7 + 0.3;
    float g = cos(uv.y * d) * 0.5 + 0.2;
    float b = cos(a + d) * 0.3 + 0.5;
    vec3 col = vec3(r, g, b);
    col = cos(col * cos(vec3(d, a, 2.5)) * 0.5 + 0.5);
    col = vec3(
        col.g * 0.56 + col.b * 0.14 + 0.12,
        col.b * 0.64 + col.r * 0.10 + 0.18,
        col.r * 0.36 + col.g * 0.26 + 0.20
    );
    col = min(max(col, vec3(0.0)), vec3(1.0));

    return vec4(col, 1.0);
}
"#;

    const AURORA_CORE_IMAGE_SHADER: &str = r#"
kernel vec4 overviewShader(float iTime, vec2 iResolution) {
    vec2 fragCoord = destCoord();
    vec2 uv = fragCoord / iResolution;

    float t = iTime * 0.14;
    float d = -t;
    float a = 0.0;

    for (float i = 0.0; i < 9.0; ++i) {
        a += cos(d + i * uv.x - a);
        d += 0.5 * sin(a + i * uv.y);
    }

    d += t;

    float r = cos(uv.x * a) * 0.7 + 0.3;
    float g = cos(uv.y * d) * 0.5 + 0.2;
    float b = cos(a + d) * 0.3 + 0.5;
    vec3 col = vec3(r, g, b);
    col = cos(col * cos(vec3(d, a, 2.5)) * 0.5 + 0.5);
    col = vec3(
        col.b * 0.54 + col.r * 0.14 + 0.18,
        col.r * 0.22 + col.g * 0.52 + 0.10,
        col.g * 0.30 + col.b * 0.68 + 0.12
    );
    col = min(max(col, vec3(0.0)), vec3(1.0));

    return vec4(col, 1.0);
}
"#;

    thread_local! {
        static TARGETS: RefCell<HashMap<ShaderTargetKey, ShaderTarget>> = RefCell::new(HashMap::new());
    }

    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    struct ShaderTargetKey {
        seed: String,
        variant: OverviewShaderVariant,
        width: usize,
        height: usize,
    }

    struct ShaderGpu {
        context: Retained<CIContext>,
        flow_kernel: Retained<CIColorKernel>,
        bands_kernel: Retained<CIColorKernel>,
        ember_kernel: Retained<CIColorKernel>,
        lagoon_kernel: Retained<CIColorKernel>,
        aurora_kernel: Retained<CIColorKernel>,
    }

    struct ShaderTarget {
        buffer: CVPixelBuffer,
        frame_bucket: Option<u64>,
    }

    pub fn shader_surface(seed: String, variant: OverviewShaderVariant) -> Div {
        div().relative().overflow_hidden().bg(rgb(0x06111f)).child(
            canvas(
                {
                    let seed = seed.clone();
                    move |bounds, window, _| {
                        render_shader_target(bounds, window.scale_factor(), &seed, variant)
                    }
                },
                move |bounds, target, window, _| {
                    if let Some(target) = target {
                        window.paint_surface(bounds, target);
                    } else {
                        window.paint_quad(fill(bounds, rgb(0x06111f)));
                    }
                    window.request_animation_frame();
                },
            )
            .absolute()
            .inset_0()
            .size_full(),
        )
    }

    fn render_shader_target(
        bounds: Bounds<Pixels>,
        scale_factor: f32,
        seed: &str,
        variant: OverviewShaderVariant,
    ) -> Option<CVPixelBuffer> {
        let width = even_device_pixels(f32::from(bounds.size.width) * scale_factor);
        let height = even_device_pixels(f32::from(bounds.size.height) * scale_factor);
        let key = ShaderTargetKey {
            seed: seed.to_string(),
            variant,
            width,
            height,
        };

        let elapsed = shader_elapsed();
        let frame_bucket = (elapsed * TARGET_FRAME_RATE).floor() as u64;
        let (target, last_frame_bucket) = TARGETS.with(|targets| {
            let mut targets = targets.borrow_mut();
            if targets.len() > 160 {
                targets.retain(|cached_key, _| cached_key == &key);
            }

            if let Some(target) = targets.get(&key) {
                return Some((target.buffer.clone(), target.frame_bucket));
            }

            let buffer = create_target(width, height)?;
            targets.insert(
                key.clone(),
                ShaderTarget {
                    buffer: buffer.clone(),
                    frame_bucket: None,
                },
            );
            Some((buffer, None))
        })?;

        if last_frame_bucket != Some(frame_bucket) {
            let time = frame_bucket as f32 / TARGET_FRAME_RATE + seed_phase(seed);
            shader_gpu().render(&target, width, height, time, variant)?;
            TARGETS.with(|targets| {
                if let Some(target) = targets.borrow_mut().get_mut(&key) {
                    target.frame_bucket = Some(frame_bucket);
                }
            });
        }

        Some(target)
    }

    fn create_target(width: usize, height: usize) -> Option<CVPixelBuffer> {
        let iosurface_properties = CFDictionary::<CFString, CFType>::from_CFType_pairs(&[]);
        let options = CFDictionary::from_CFType_pairs(&[
            (
                CFString::from(CVPixelBufferKeys::MetalCompatibility),
                CFBoolean::true_value().as_CFType(),
            ),
            (
                CFString::from(CVPixelBufferKeys::IOSurfaceProperties),
                iosurface_properties.as_CFType(),
            ),
        ]);

        CVPixelBuffer::new(
            kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
            width,
            height,
            Some(&options),
        )
        .ok()
    }

    fn shader_gpu() -> &'static ShaderGpu {
        static GPU: OnceLock<ShaderGpu> = OnceLock::new();
        GPU.get_or_init(|| {
            let software_renderer: Retained<AnyObject> = NSNumber::new_bool(false).into();
            let options: Retained<NSDictionary<CIContextOption, AnyObject>> =
                NSDictionary::from_slices(
                    &[unsafe { kCIContextUseSoftwareRenderer }],
                    &[&*software_renderer],
                );
            let flow_source = NSString::from_str(FLOW_CORE_IMAGE_SHADER);
            let bands_source = NSString::from_str(BANDS_CORE_IMAGE_SHADER);
            let ember_source = NSString::from_str(EMBER_CORE_IMAGE_SHADER);
            let lagoon_source = NSString::from_str(LAGOON_CORE_IMAGE_SHADER);
            let aurora_source = NSString::from_str(AURORA_CORE_IMAGE_SHADER);
            #[allow(deprecated)]
            let flow_kernel = unsafe { CIColorKernel::kernelWithString(&flow_source) }
                .expect("overview flow shader must compile as a Core Image GPU kernel");
            #[allow(deprecated)]
            let bands_kernel = unsafe { CIColorKernel::kernelWithString(&bands_source) }
                .expect("overview bands shader must compile as a Core Image GPU kernel");
            #[allow(deprecated)]
            let ember_kernel = unsafe { CIColorKernel::kernelWithString(&ember_source) }
                .expect("overview ember shader must compile as a Core Image GPU kernel");
            #[allow(deprecated)]
            let lagoon_kernel = unsafe { CIColorKernel::kernelWithString(&lagoon_source) }
                .expect("overview lagoon shader must compile as a Core Image GPU kernel");
            #[allow(deprecated)]
            let aurora_kernel = unsafe { CIColorKernel::kernelWithString(&aurora_source) }
                .expect("overview aurora shader must compile as a Core Image GPU kernel");
            let context = unsafe { CIContext::contextWithOptions(Some(&options)) };

            ShaderGpu {
                context,
                flow_kernel,
                bands_kernel,
                ember_kernel,
                lagoon_kernel,
                aurora_kernel,
            }
        })
    }

    impl ShaderGpu {
        fn render(
            &self,
            target: &CVPixelBuffer,
            width: usize,
            height: usize,
            time: f32,
            variant: OverviewShaderVariant,
        ) -> Option<Retained<CIImage>> {
            let time: Retained<AnyObject> = NSNumber::new_f32(time).into();
            let resolution: Retained<AnyObject> =
                unsafe { CIVector::vectorWithX_Y(width as CGFloat, height as CGFloat).into() };
            let args = NSArray::from_retained_slice(&[time, resolution]);
            let extent = CGRect::new(
                CGPoint::new(0.0, 0.0),
                CGSize::new(width as CGFloat, height as CGFloat),
            );
            let image = unsafe {
                self.kernel(variant)
                    .applyWithExtent_arguments(extent, &args)?
            };
            unsafe {
                self.context
                    .render_toCVPixelBuffer(&image, objc_cv_pixel_buffer(target));
            }

            Some(image)
        }

        fn kernel(&self, variant: OverviewShaderVariant) -> &CIColorKernel {
            match variant {
                OverviewShaderVariant::Flow => &self.flow_kernel,
                OverviewShaderVariant::Bands => &self.bands_kernel,
                OverviewShaderVariant::Ember => &self.ember_kernel,
                OverviewShaderVariant::Lagoon => &self.lagoon_kernel,
                OverviewShaderVariant::Aurora => &self.aurora_kernel,
            }
        }
    }

    fn even_device_pixels(value: f32) -> usize {
        let pixels = value.ceil().max(2.0) as usize;
        pixels + pixels % 2
    }

    unsafe fn objc_cv_pixel_buffer(buffer: &CVPixelBuffer) -> &ObjcCVPixelBuffer {
        &*(buffer.as_concrete_TypeRef() as *const ObjcCVPixelBuffer)
    }

    fn shader_elapsed() -> f32 {
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_secs_f32()
    }

    fn seed_phase(seed: &str) -> f32 {
        let hash = seed.bytes().fold(2166136261u32, |acc, byte| {
            acc.wrapping_mul(16777619) ^ byte as u32
        });
        hash as f32 / u32::MAX as f32 * 7.0
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::OverviewShaderVariant;
    use gpui::prelude::*;
    use gpui::*;

    pub fn shader_surface(_seed: String, _variant: OverviewShaderVariant) -> Div {
        div().bg(rgb(0x06111f))
    }
}
