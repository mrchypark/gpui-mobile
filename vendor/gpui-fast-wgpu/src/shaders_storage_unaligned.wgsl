// Storage-buffer transport for adapters that require 16-byte binding ranges.
// Path records remain packed at 26 words on the host and are decoded from vec4 words.

@group(1) @binding(0) var<storage, read> b_quads: array<Quad>;
@group(1) @binding(0) var<storage, read> b_shadows: array<Shadow>;
@group(1) @binding(0) var<storage, read> b_path_vertices: array<vec4<u32>>;
@group(1) @binding(0) var<storage, read> b_path_sprites: array<PathSprite>;
@group(1) @binding(0) var<storage, read> b_underlines: array<Underline>;
@group(1) @binding(0) var<storage, read> b_mono_sprites: array<MonochromeSprite>;
@group(1) @binding(0) var<storage, read> b_poly_sprites: array<PolychromeSprite>;

fn load_quad(instance_id: u32) -> Quad {
    return b_quads[instance_id];
}

fn load_shadow(instance_id: u32) -> Shadow {
    return b_shadows[instance_id];
}

fn load_path_vertex_word(vertex_id: u32, word_offset: u32) -> u32 {
    let word_index = vertex_id * 26u + word_offset;
    return b_path_vertices[word_index / 4u][word_index % 4u];
}

fn load_path_vertex(vertex_id: u32) -> PathRasterizationVertex {
    return PathRasterizationVertex(
        vec2<f32>(
            bitcast<f32>(load_path_vertex_word(vertex_id, 0u)),
            bitcast<f32>(load_path_vertex_word(vertex_id, 1u)),
        ),
        vec2<f32>(
            bitcast<f32>(load_path_vertex_word(vertex_id, 2u)),
            bitcast<f32>(load_path_vertex_word(vertex_id, 3u)),
        ),
        Background(
            load_path_vertex_word(vertex_id, 4u),
            load_path_vertex_word(vertex_id, 5u),
            Hsla(
                bitcast<f32>(load_path_vertex_word(vertex_id, 6u)),
                bitcast<f32>(load_path_vertex_word(vertex_id, 7u)),
                bitcast<f32>(load_path_vertex_word(vertex_id, 8u)),
                bitcast<f32>(load_path_vertex_word(vertex_id, 9u)),
            ),
            bitcast<f32>(load_path_vertex_word(vertex_id, 10u)),
            array<LinearColorStop, 2>(
                LinearColorStop(
                    Hsla(
                        bitcast<f32>(load_path_vertex_word(vertex_id, 11u)),
                        bitcast<f32>(load_path_vertex_word(vertex_id, 12u)),
                        bitcast<f32>(load_path_vertex_word(vertex_id, 13u)),
                        bitcast<f32>(load_path_vertex_word(vertex_id, 14u)),
                    ),
                    bitcast<f32>(load_path_vertex_word(vertex_id, 15u)),
                ),
                LinearColorStop(
                    Hsla(
                        bitcast<f32>(load_path_vertex_word(vertex_id, 16u)),
                        bitcast<f32>(load_path_vertex_word(vertex_id, 17u)),
                        bitcast<f32>(load_path_vertex_word(vertex_id, 18u)),
                        bitcast<f32>(load_path_vertex_word(vertex_id, 19u)),
                    ),
                    bitcast<f32>(load_path_vertex_word(vertex_id, 20u)),
                ),
            ),
            load_path_vertex_word(vertex_id, 21u),
        ),
        Bounds(
            vec2<f32>(
                bitcast<f32>(load_path_vertex_word(vertex_id, 22u)),
                bitcast<f32>(load_path_vertex_word(vertex_id, 23u)),
            ),
            vec2<f32>(
                bitcast<f32>(load_path_vertex_word(vertex_id, 24u)),
                bitcast<f32>(load_path_vertex_word(vertex_id, 25u)),
            ),
        ),
    );
}

fn load_path_sprite(instance_id: u32) -> PathSprite {
    return b_path_sprites[instance_id];
}

fn load_underline(instance_id: u32) -> Underline {
    return b_underlines[instance_id];
}

fn load_mono_sprite(instance_id: u32) -> MonochromeSprite {
    return b_mono_sprites[instance_id];
}

fn load_poly_sprite(instance_id: u32) -> PolychromeSprite {
    return b_poly_sprites[instance_id];
}
