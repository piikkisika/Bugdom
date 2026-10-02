// Glowing effects (`STATUS_BIT_GLOW`): a texture times a colour and the
// vertex colour, added to what is behind it scaled by alpha
// (`glBlendFunc(GL_SRC_ALPHA, GL_ONE)`). No fog (`STATUS_BIT_NOFOG`) and no
// lighting: what lighting there is comes in with the colour.

#import bevy_pbr::forward_io::VertexOutput
#ifdef TONEMAP_IN_SHADER
#import bevy_core_pipeline::tonemapping::tone_mapping
#import bevy_pbr::mesh_view_bindings::view
#endif

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> color: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var base_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var base_sampler: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var out = color;
#ifdef VERTEX_UVS_A
    out *= textureSample(base_texture, base_sampler, in.uv);
#endif
#ifdef VERTEX_COLORS
    out *= in.color;
#endif
#ifdef TONEMAP_IN_SHADER
    out = tone_mapping(out, view.color_grading);
#endif
    // `AlphaMode::Add` blends as premultiplied alpha (one, one minus source
    // alpha), so premultiply here and leave nothing to subtract.
    return vec4(out.rgb * out.a, 0.0);
}
