// Terrain: tile texture array (layer in uv_b.x) times the prelit vertex
// colour, unlit, then Bevy's fog and post-lighting processing.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::main_pass_post_lighting_processing,
    forward_io::{VertexOutput, FragmentOutput},
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var tiles_texture: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var tiles_sampler: sampler;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    // Base colour times vertex colour.
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    let layer = i32(round(in.uv_b.x));
    pbr_input.material.base_color *= textureSample(tiles_texture, tiles_sampler, in.uv, layer);

    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, pbr_input.material.base_color);
    return out;
}
