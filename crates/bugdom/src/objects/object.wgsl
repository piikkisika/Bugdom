// Objects: the original's fixed-function lighting. Ambient plus two
// directional fill lights, summed and clamped to 1 in display (sRGB) space
// and multiplied with the surface colour there, as OpenGL did. Unlit
// objects skip the lighting. Then Bevy's fog and post-lighting processing.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
}
#import bevy_render::color_operations::{linear_to_srgb, srgb_to_linear}

struct ObjectLighting {
    ambient: vec4<f32>,
    // Light colour times brightness, as display values.
    colors: array<vec4<f32>, 2>,
    // Unit vectors toward each light.
    directions: array<vec4<f32>, 2>,
    // 1 for lit, 0 for unlit (`STATUS_BIT_NULLSHADER`).
    lit: f32,
    // 1 to make blending a dark colour darken as it did in display space.
    display_alpha: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> lighting: ObjectLighting;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    var color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    if lighting.lit > 0.5 {
        let n = pbr_input.N;
        var light = lighting.ambient.rgb;
        for (var i = 0; i < 2; i++) {
            light += lighting.colors[i].rgb * max(dot(n, lighting.directions[i].xyz), 0.0);
        }
        light = min(light, vec3(1.0));
        color = vec4(srgb_to_linear(linear_to_srgb(color.rgb) * light), color.a);
    }
    if lighting.display_alpha > 0.5 {
        // Blending black over d leaves d * (1 - a) in display space, which
        // is d * (1 - a)^2.2 in linear space.
        color.a = 1.0 - pow(1.0 - color.a, 2.2);
    }

    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, color);
    return out;
}
