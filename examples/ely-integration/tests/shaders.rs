const SHADERS: &str = include_str!("../../../vendor/gpui-fast-wgpu/src/shaders.wgsl");
const STORAGE: &str = include_str!("../../../vendor/gpui-fast-wgpu/src/shaders_storage.wgsl");
const ALIGNED: &str =
    include_str!("../../../vendor/gpui-fast-wgpu/src/shaders_storage_unaligned.wgsl");
const SUBPIXEL: &str = include_str!("../../../vendor/gpui-fast-wgpu/src/shaders_subpixel.wgsl");

#[test]
fn storage_shader_variants_validate() {
    for storage in [STORAGE, ALIGNED] {
        for subpixel in [false, true] {
            let source = if subpixel {
                format!("enable dual_source_blending;\n{SHADERS}{storage}{SUBPIXEL}")
            } else {
                format!("{SHADERS}{storage}")
            };
            let module = naga::front::wgsl::parse_str(&source).expect("parse GPU shader");
            let capabilities = if subpixel {
                naga::valid::Capabilities::DUAL_SOURCE_BLENDING
            } else {
                naga::valid::Capabilities::empty()
            };
            naga::valid::Validator::new(naga::valid::ValidationFlags::all(), capabilities)
                .validate(&module)
                .expect("validate GPU shader");
        }
    }
}
