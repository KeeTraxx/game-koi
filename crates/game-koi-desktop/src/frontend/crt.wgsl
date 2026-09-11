// The CRT display pass: reads the scaled picture and writes it to the surface.
//
// Paired with `crt.rs`, which owns the pipeline and keeps `Params` in step with this
// struct — the byte layout is packed by hand there, so the two have to be changed
// together.
//
// Everything here works in *physical pixels* of the surface, via `@builtin(position)`.
// That is the whole point of doing this as a post-process: a scanline belongs to the
// display, not to the 160x144 picture, so it has to be measured in screen rows.

struct Params {
    // Where the picture sits in the surface: x, y, width, height, in physical pixels.
    // The scaler letterboxes to keep 10:9, so this is usually smaller than the window
    // and the difference is black border that must be left alone.
    image: vec4<f32>,
    // How many physical rows one emulated scanline covers. Fractional whenever the
    // window is not an exact multiple of 144 tall.
    period: f32,
    // How deep the dark lines go, 0.0 to 1.0.
    strength: f32,
    // Pads the struct to 32 bytes. A uniform struct's size must be a multiple of 16.
    padding: vec2<f32>,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;

// A full-screen triangle generated from the vertex index, so there is no vertex buffer
// to allocate or bind. Three vertices at (-1,-1), (3,-1), (-1,3) in clip space: the
// triangle overhangs the viewport on two sides, and the part that lands inside covers
// it exactly. Cheaper than two triangles and avoids a seam along the diagonal.
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(index) / 2) * 4.0 - 1.0;
    let y = f32(i32(index) & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // A 1:1 blit: the intermediate is the same size as the surface, so the pixel centre
    // `position.xy` lands exactly on a texel centre and the sampler returns that texel
    // untouched. The sampler is here for the effects that will want to sample *off*
    // centre — curvature, bloom — rather than for this one.
    let uv = position.xy / vec2<f32>(textureDimensions(source));
    var colour = textureSample(source, source_sampler, uv).rgb;

    let inside = all(position.xy >= params.image.xy)
        && all(position.xy < params.image.xy + params.image.zw);

    // Below two rows per line there is nowhere to put a dark row that is not the line
    // itself, so the effect would just halve the brightness of the whole picture.
    if inside && params.period >= 2.0 {
        // Phase measured from the top of the *picture*, not the window, so the stripes
        // stay aligned with the emulated lines when the window is letterboxed.
        //
        // The 0.5 turns the pixel *centre* that `position` reports back into the row's
        // own index. Without it the cosine is sampled half a row out of step, and at the
        // common case of four rows per line it lands on +-pi/4 every time: the extremes
        // are never reached, so the profile comes out as two bright rows and two dark
        // ones — a square wave — and `strength` silently means about half what it says.
        // Anchored to the row, four rows per line give one lit row, one dark one, and
        // two between, which is the shape a beam actually had.
        let row = position.y - 0.5 - params.image.y;
        let phase = 6.283185307 * row / params.period;
        // A cosine rather than a hard every-other-row test. At a fractional period a
        // step function beats against the pixel grid and produces coarse bands that
        // crawl as the window resizes; this just softens instead.
        let mask = 1.0 - params.strength * 0.5 * (1.0 - cos(phase));

        // Put back the brightness the mask took out. Averaged over one period the mask
        // is `1 - strength/2`, so dividing by that keeps the mean level where it was.
        // Without this the effect is indistinguishable from turning the backlight down,
        // which is not what a CRT looked like: its lit lines were *brighter* than the
        // average, and the dark gaps were what your eye averaged against. Highlights
        // clip at white, which is roughly what the phosphor did anyway.
        colour = colour * mask / (1.0 - params.strength * 0.5);
    }

    return vec4<f32>(colour, 1.0);
}
