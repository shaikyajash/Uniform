// The hero's light: three sine waves, one per colour, drawn as thin glowing lines. They leave the
// left side as a rainbow and close into one white ray on the right. Many coins in, one payout out.
//
// The "WebGL Shader" hero by designali-in on 21st.dev, mirrored left to right. Kept exact,
// including a quirk that gives it its look: the original passes the screen size in CSS pixels but
// draws at the device's pixel ratio, so on a retina screen (ratio 2) the picture is twice as large
// and centred a quarter of the way in. Here that ratio of 2 is fixed, so every screen sees what a
// retina Mac sees. Plain WebGL: one full-screen quad and a fragment shader.

const VERTEX = `
attribute vec2 position;
void main() {
  gl_Position = vec4(position, 0.0, 1.0);
}`;

const FRAGMENT = `
precision highp float;
// The canvas size in CSS pixels, and device pixels per CSS pixel.
uniform vec2 resolution;
uniform float pixelRatio;
uniform float time;

const float X_SCALE = 1.0;
const float Y_SCALE = 0.5;
const float DISTORTION = 0.05;
// The original as seen on a retina screen.
const float LOOK_RATIO = 2.0;
// Where the wave runs, as a fraction of the height from the bottom: just under the buttons, clear
// of the logo strip. The original's is 0.25, which hides the left side under the strip.
const float CENTER_Y = 0.44;

void main() {
  vec2 css = gl_FragCoord.xy / pixelRatio;
  // Mirrored: the rainbow is on the left, the single ray on the right.
  css.x = resolution.x - css.x;
  vec2 p = (css * LOOK_RATIO * 2.0 - resolution) / min(resolution.x, resolution.y);
  p.y = (css.y - resolution.y * CENTER_Y) * LOOK_RATIO * 2.0 / min(resolution.x, resolution.y);

  float d = length(p) * DISTORTION;
  float rx = p.x * (1.0 + d);
  float gx = p.x;
  float bx = p.x * (1.0 - d);

  float r = 0.05 / abs(p.y + sin((rx + time) * X_SCALE) * Y_SCALE);
  float g = 0.05 / abs(p.y + sin((gx + time) * X_SCALE) * Y_SCALE);
  float b = 0.05 / abs(p.y + sin((bx + time) * X_SCALE) * Y_SCALE);

  gl_FragColor = vec4(r, g, b, 1.0);
}`;

/** The original's speed: 0.01 per frame at 60 frames a second. */
const SPEED = 0.6;
/** The lines are one or two pixels wide: draw at the screen's full sharpness, up to 2. */
const MAX_PIXEL_RATIO = 2;

function compile(gl: WebGLRenderingContext, type: number, source: string): WebGLShader {
  const shader = gl.createShader(type);
  if (!shader) throw new Error("could not create a shader");
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    throw new Error(gl.getShaderInfoLog(shader) ?? "shader did not compile");
  }
  return shader;
}

/**
 * Draws the ray on `canvas` until the returned function is called. With `still`, one frame and
 * no motion. Throws if there is no WebGL.
 */
export function startRay(canvas: HTMLCanvasElement, still: boolean): () => void {
  const gl = canvas.getContext("webgl", { antialias: false, alpha: false, powerPreference: "low-power" });
  if (!gl) throw new Error("no WebGL");

  const program = gl.createProgram();
  if (!program) throw new Error("could not create a program");
  gl.attachShader(program, compile(gl, gl.VERTEX_SHADER, VERTEX));
  gl.attachShader(program, compile(gl, gl.FRAGMENT_SHADER, FRAGMENT));
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    throw new Error(gl.getProgramInfoLog(program) ?? "program did not link");
  }
  gl.useProgram(program);

  const buffer = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]), gl.STATIC_DRAW);
  const position = gl.getAttribLocation(program, "position");
  gl.enableVertexAttribArray(position);
  gl.vertexAttribPointer(position, 2, gl.FLOAT, false, 0, 0);

  const resolution = gl.getUniformLocation(program, "resolution");
  const pixelRatio = gl.getUniformLocation(program, "pixelRatio");
  const time = gl.getUniformLocation(program, "time");

  function resize() {
    const ratio = Math.min(window.devicePixelRatio || 1, MAX_PIXEL_RATIO);
    const width = Math.max(1, Math.round(canvas.clientWidth * ratio));
    const height = Math.max(1, Math.round(canvas.clientHeight * ratio));
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    gl!.viewport(0, 0, width, height);
    gl!.uniform2f(resolution, width / ratio, height / ratio);
    gl!.uniform1f(pixelRatio, ratio);
  }

  // The original's first frame.
  const STILL_TIME = 0;
  const started = performance.now();
  let frame = 0;
  function draw() {
    const seconds = still ? STILL_TIME : ((performance.now() - started) / 1000) * SPEED;
    gl!.uniform1f(time, seconds);
    gl!.drawArrays(gl!.TRIANGLES, 0, 6);
    if (!still) frame = requestAnimationFrame(draw);
  }

  const observer = new ResizeObserver(() => {
    resize();
    if (still) draw();
  });
  observer.observe(canvas);
  resize();
  draw();

  return () => {
    cancelAnimationFrame(frame);
    observer.disconnect();
    gl.getExtension("WEBGL_lose_context")?.loseContext();
  };
}
