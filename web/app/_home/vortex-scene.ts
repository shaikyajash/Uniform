// The hero's 3D vortex, drawn with three.js.
//
// Four things move in it:
// - strands: 240 lines along the vortex shape. On load they grow up from the floor in 2 seconds.
// - dots: 8,000 small squares riding the strands and flickering.
// - comets: 10 short bright trails that race up a strand now and then.
// - when a comet hits a dot, the dot pops, its neighbours are pushed away and spring back, and
//   a ripple runs through the strands near it.

import * as THREE from "three";
import { laneOf, TAU, writePoint } from "./vortex-shape";

const STRANDS = 240;
const POINTS = 400;
const SEGMENTS = POINTS - 1;
const DOTS = 8_000;
const DOT_SIZE = 0.02;
const DOT_GLOW = 4.2;
const COMETS = 10;
const COMET_SPEED = 0.09;
const COMET_GLOW = 0.6;
const COMET_TAIL = 19;
const COMET_DELAY = 8;
const COMET_COLOR = "#eca8d6";
/** How hard a comet hits a dot, 0 to 1. */
const HIT_FORCE = 0.6;
const FLOW_SPEED = 0.1;
const FIELD_OF_VIEW = 59;
/** Strands are recomputed 30 times a second, not every frame. */
const STRAND_STEP = 1 / 30;
const MAX_WAVES = 16;

/** 0 before `from`, 1 after `to`, easing out in between. */
function easeIn(time: number, from: number, to: number): number {
  if (time <= from) return 0;
  if (time >= to) return 1;
  const left = 1 - (time - from) / (to - from);
  return 1 - left * left * left;
}

const clamp = (value: number, low: number, high: number) => Math.min(Math.max(value, low), high);

type Strand = { lane: number; speed: number; pulse: number; wobblePhase: number; points: Float32Array };
type Dot = { s: number; strand: number; pulse: number; flickerRate: number; bright: number };
type Comet = {
  bright: number;
  lane: number;
  pace: number;
  wobblePhase: number;
  base: number;
  speed: number;
  boost: number;
  racing: boolean;
  s: number;
  idle: number;
  idleFor: number;
  trail: Float32Array;
  trailColor: Float32Array;
  geometry: THREE.BufferGeometry;
  head: THREE.Sprite;
};
type Wave = { active: boolean; x: number; y: number; z: number; at: number; amp: number };

/** Starts the vortex on `canvas`. Returns a stop function. */
export function startVortex(canvas: HTMLCanvasElement, still: boolean): () => void {
  const renderer = new THREE.WebGLRenderer({ canvas, antialias: true, alpha: true, powerPreference: "high-performance" });
  renderer.setClearColor(0x000000, 0);
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 1.5));
  renderer.toneMapping = THREE.ReinhardToneMapping;
  renderer.toneMappingExposure = 1.25;

  const scene = new THREE.Scene();
  const camera = new THREE.PerspectiveCamera(FIELD_OF_VIEW, 1, 0.1, 500);
  const target = new THREE.Vector3(0, 5, 0);
  const distance = 5 / Math.tan(((67 * Math.PI) / 180) / 2);
  camera.position.copy(target).addScaledVector(new THREE.Vector3(3.4, -0.6, 10).normalize(), distance);
  camera.lookAt(target);

  const disposables: { dispose(): void }[] = [];
  const keep = <T extends { dispose(): void }>(thing: T) => (disposables.push(thing), thing);

  const strandColor = new THREE.Color("#ffffff");
  const dotColor = new THREE.Color("#ffffff");
  const cometColor = new THREE.Color(COMET_COLOR);

  // Strands: one buffer of line segments for all of them.
  const strandPositions = new Float32Array(STRANDS * SEGMENTS * 2 * 3);
  const strandColors = new Float32Array(strandPositions.length);
  const strandGeometry = keep(new THREE.BufferGeometry());
  strandGeometry.setAttribute("position", new THREE.BufferAttribute(strandPositions, 3).setUsage(THREE.DynamicDrawUsage));
  strandGeometry.setAttribute("color", new THREE.BufferAttribute(strandColors, 3).setUsage(THREE.DynamicDrawUsage));
  const strandMaterial = keep(
    new THREE.LineBasicMaterial({
      vertexColors: true,
      transparent: true,
      opacity: 0.5,
      blending: THREE.AdditiveBlending,
      depthWrite: false,
    }),
  );
  const strandMesh = new THREE.LineSegments(strandGeometry, strandMaterial);
  strandMesh.frustumCulled = false;
  scene.add(strandMesh);

  const strands: Strand[] = Array.from({ length: STRANDS }, (_, i) => ({
    lane: laneOf(i, STRANDS),
    speed: 0.95 + 0.1 * Math.random(),
    pulse: Math.random() * TAU,
    wobblePhase: Math.random() * TAU,
    points: new Float32Array(POINTS * 3),
  }));
  const pointColors = new Float32Array(POINTS * 3);

  // Dots: one instanced square each. Half of them sit around the waist.
  const dots: Dot[] = Array.from({ length: DOTS }, () => ({
    s: Math.random() < 0.5 ? 0.2 + 0.4 * Math.random() : 0.05 + 0.9 * Math.random(),
    strand: Math.floor(Math.random() * STRANDS),
    pulse: Math.random() * TAU,
    flickerRate: 0.15 + 4.5 * Math.random(),
    bright: 0.04 + Math.random() ** 1.5 * 0.96,
  }));
  const dotAt = new Float32Array(DOTS * 3);
  const dotOffset = new Float32Array(DOTS * 3);
  const dotVelocity = new Float32Array(DOTS * 3);
  const dotScale = new Float32Array(DOTS).fill(1);
  const dotScaleVelocity = new Float32Array(DOTS);
  const dotPopAge = new Float32Array(DOTS);
  const dotFlash = new Float32Array(DOTS);
  const dotShown = new Float32Array(DOTS).fill(1);
  const dotColors = new Float32Array(DOTS * 3);
  let springing = false;

  const dotMaterial = keep(
    new THREE.MeshBasicMaterial({
      color: 0xffffff,
      transparent: true,
      opacity: 0.9,
      blending: THREE.AdditiveBlending,
      depthWrite: false,
    }),
  );
  const dotMesh = new THREE.InstancedMesh(keep(new THREE.PlaneGeometry(1, 1)), dotMaterial, DOTS);
  dotMesh.instanceMatrix.setUsage(THREE.DynamicDrawUsage);
  dotMesh.instanceColor = new THREE.InstancedBufferAttribute(dotColors, 3).setUsage(THREE.DynamicDrawUsage);
  dotMesh.frustumCulled = false;
  scene.add(dotMesh);

  // Comets: a fading line plus a soft glowing head.
  const glow = keep(radialTexture());
  const comets: Comet[] = Array.from({ length: COMETS }, (_, i) => {
    const trail = new Float32Array(COMET_TAIL * 3);
    const trailColor = new Float32Array(COMET_TAIL * 3);
    const geometry = keep(new THREE.BufferGeometry());
    geometry.setAttribute("position", new THREE.BufferAttribute(trail, 3).setUsage(THREE.DynamicDrawUsage));
    geometry.setAttribute("color", new THREE.BufferAttribute(trailColor, 3).setUsage(THREE.DynamicDrawUsage));
    const line = new THREE.Line(
      geometry,
      keep(
        new THREE.LineBasicMaterial({
          vertexColors: true,
          transparent: true,
          opacity: 0.9,
          blending: THREE.AdditiveBlending,
          depthWrite: false,
        }),
      ),
    );
    line.frustumCulled = false;
    const head = new THREE.Sprite(
      keep(
        new THREE.SpriteMaterial({
          map: glow,
          transparent: true,
          opacity: 0,
          blending: THREE.AdditiveBlending,
          depthWrite: false,
          color: new THREE.Color(1.2 * cometColor.r, 1.2 * cometColor.g, 1.2 * cometColor.b),
        }),
      ),
    );
    head.scale.set(0.35, 0.35, 1);
    scene.add(line, head);
    const strand = strands[Math.floor(Math.random() * STRANDS)];
    const base = COMET_SPEED * (0.7 + 0.6 * Math.random());
    return {
      bright: 0.7 + 0.3 * Math.random(),
      lane: strand.lane,
      pace: strand.speed,
      wobblePhase: strand.wobblePhase,
      base,
      speed: base,
      boost: 0,
      racing: false,
      s: 0,
      idle: 0,
      idleFor: 0.4 + (i / COMETS) * COMET_DELAY,
      trail,
      trailColor,
      geometry,
      head,
    };
  });

  // Ripples started by comet hits.
  const waves: Wave[] = Array.from({ length: MAX_WAVES }, () => ({ active: false, x: 0, y: 0, z: 0, at: 0, amp: 1 }));
  let rippling = false;

  /** Moves the point at `i` in `points` outwards from any ripple passing it. */
  function ripple(points: Float32Array, i: number, time: number) {
    let dx = 0;
    let dy = 0;
    let dz = 0;
    let any = false;
    for (const wave of waves) {
      if (!wave.active) continue;
      const age = time - wave.at;
      if (age > 2.5) {
        wave.active = false;
        continue;
      }
      any = true;
      const ox = points[i] - wave.x;
      const oy = points[i + 1] - wave.y;
      const oz = points[i + 2] - wave.z;
      const far = Math.sqrt(ox * ox + oy * oy + oz * oz);
      if (far < 0.001 || far > 3) continue;
      const fromFront = Math.abs(far - 5 * age);
      if (fromFront > 1.2) continue;
      const push =
        0.5 * wave.amp * 0.04 * Math.cos(((fromFront / 1.2) * Math.PI) / 2) * Math.exp(-age / 0.8) / Math.max(far, 0.3);
      dx += (ox / far) * push;
      dy += (oy / far) * push;
      dz += (oz / far) * push;
    }
    if (!any) rippling = false;
    points[i] += dx;
    points[i + 1] += dy;
    points[i + 2] += dz;
  }

  function drawStrand(strand: Strand, index: number, time: number, spinBase: number, grow: number) {
    const spin = spinBase * strand.speed;
    const level = 0.15 + 1.5 * 0.5;
    const shimmer = Math.min(0.25 * (0.9 + 0.1 * Math.sin(0.18 * time + strand.pulse)), 0.7) * Math.min(3 * grow, 1);
    const tip = grow;
    const soft = 0.15;
    const { points } = strand;
    for (let i = 0; i < POINTS; i++) {
      const s = i / SEGMENTS;
      const k = i * 3;
      writePoint(points, k, s, strand.lane, spin, strand.wobblePhase, time);
      if (rippling) ripple(points, k, time);
      // Fade both ends, and hide what has not grown in yet.
      let ends = 1;
      if (s < 0.15) ends = (s / 0.15) ** 2;
      else if (s > 0.85) ends = ((1 - s) / 0.15) ** 2;
      let grown = 1;
      if (s > tip) grown = 0;
      else if (s > tip - soft) grown = ((tip - s) / soft) ** 2;
      const light = ends * level * grown * shimmer;
      pointColors[k] = strandColor.r * light;
      pointColors[k + 1] = strandColor.g * light;
      pointColors[k + 2] = strandColor.b * light;
    }
    let at = index * SEGMENTS * 6;
    for (let i = 0; i < SEGMENTS; i++) {
      for (const k of [i * 3, (i + 1) * 3]) {
        strandPositions[at] = points[k];
        strandPositions[at + 1] = points[k + 1];
        strandPositions[at + 2] = points[k + 2];
        strandColors[at] = pointColors[k];
        strandColors[at + 1] = pointColors[k + 1];
        strandColors[at + 2] = pointColors[k + 2];
        at += 3;
      }
    }
  }

  /** Dots knocked aside spring back; popped dots swell back to size. */
  function settle(dt: number) {
    let moving = false;
    for (let i = 0; i < DOTS; i++) {
      const k = i * 3;
      for (let axis = 0; axis < 3; axis++) {
        const pull = -50 * dotOffset[k + axis] - 9 * dotVelocity[k + axis];
        dotVelocity[k + axis] += pull * dt;
        dotOffset[k + axis] += dotVelocity[k + axis] * dt;
      }
      const offset = dotOffset[k] ** 2 + dotOffset[k + 1] ** 2 + dotOffset[k + 2] ** 2;
      const speed = dotVelocity[k] ** 2 + dotVelocity[k + 1] ** 2 + dotVelocity[k + 2] ** 2;
      if (offset < 1e-8 && speed < 1e-8) {
        dotOffset.fill(0, k, k + 3);
        dotVelocity.fill(0, k, k + 3);
      } else moving = true;
      const pull = -65 * (dotScale[i] - 1) - 11 * dotScaleVelocity[i];
      dotScaleVelocity[i] += pull * dt;
      dotScale[i] += dotScaleVelocity[i] * dt;
      if (Math.abs(dotScale[i] - 1) < 0.001 && Math.abs(dotScaleVelocity[i]) < 0.001) {
        dotScale[i] = 1;
        dotScaleVelocity[i] = 0;
      } else moving = true;
    }
    springing = moving;
  }

  const matrix = new THREE.Matrix4();
  function drawDots(dt: number, time: number, spinBase: number, fadeIn: number) {
    for (let i = 0; i < DOTS; i++) {
      const dot = dots[i];
      const strand = strands[dot.strand];
      const k = i * 3;
      writePoint(dotAt, k, dot.s, strand.lane, spinBase * strand.speed, strand.wobblePhase, time);
      if (dotPopAge[i] > 0) {
        dotPopAge[i] += dt;
        const age = dotPopAge[i];
        if (age < 0.6) {
          const t = age / 0.6;
          dotShown[i] = (1 + 0.3 * (1 - t)) * (1 - t * t);
          dotFlash[i] = 6 * (1 - t * t) * (1 - t * t);
        } else {
          dotShown[i] = 0;
          dotFlash[i] = 0;
        }
        if (age > 8) {
          dotPopAge[i] = 0;
          dotShown[i] = 1;
          dotFlash[i] = 0;
        }
      }
      const size = DOT_SIZE * dotScale[i] * dotShown[i];
      matrix.makeScale(size, size, size);
      matrix.setPosition(dotAt[k] + dotOffset[k], dotAt[k + 1] + dotOffset[k + 1], dotAt[k + 2] + dotOffset[k + 2]);
      dotMesh.setMatrixAt(i, matrix);
      const flicker = 0.08 + 0.92 * Math.max(0, Math.sin(time * dot.flickerRate + dot.pulse)) ** 2.5;
      const swell = dotScale[i] > 1.02 ? 1 + (dotScale[i] - 1) * 0.5 : 1;
      const light = dot.bright * flicker * DOT_GLOW * swell * fadeIn * (1 + dotFlash[i]) * dotShown[i];
      dotColors[k] = dotColor.r * light;
      dotColors[k + 1] = dotColor.g * light;
      dotColors[k + 2] = dotColor.b * light;
    }
    dotMesh.instanceMatrix.needsUpdate = true;
    dotMesh.instanceColor!.needsUpdate = true;
    dotMaterial.opacity = 0.9 * fadeIn;
  }

  function moveComet(comet: Comet, time: number, dt: number, spinBase: number, fadeIn: number) {
    const material = comet.head.material;
    if (!comet.racing) {
      material.opacity = 0;
      if (fadeIn < 0.3) return;
      comet.idle += dt;
      if (comet.idle > comet.idleFor) {
        const strand = strands[Math.floor(Math.random() * STRANDS)];
        comet.racing = true;
        comet.s = 0.03;
        comet.base = COMET_SPEED * (0.7 + 0.6 * Math.random());
        comet.speed = comet.base;
        comet.boost = 0;
        comet.lane = strand.lane;
        comet.pace = strand.speed;
        comet.wobblePhase = strand.wobblePhase;
      }
      return;
    }
    let boostScale = 1;
    if (comet.boost > 0) {
      comet.boost -= dt;
      boostScale = comet.boost <= 0 ? 1 : 1 + 0.6 * (comet.boost / 0.4);
      comet.boost = Math.max(comet.boost, 0);
      comet.speed = comet.base * boostScale;
    }
    comet.s += dt * comet.speed;
    if (comet.s > 0.95) {
      comet.racing = false;
      comet.idle = 0;
      comet.idleFor = COMET_DELAY * (0.6 + 0.8 * Math.random());
      comet.trailColor.fill(0);
      comet.geometry.attributes.color.needsUpdate = true;
      material.opacity = 0;
      return;
    }
    const spin = spinBase * comet.pace;
    const edges = clamp((comet.s - 0.03) / 0.1, 0, 1) * clamp((0.95 - comet.s) / 0.1, 0, 1);
    for (let i = 0; i < COMET_TAIL; i++) {
      const k = i * 3;
      writePoint(comet.trail, k, clamp(comet.s - 0.005 * i, 0.005, 0.995), comet.lane, spin, comet.wobblePhase, time);
      const light = comet.bright * COMET_GLOW * (1 - i / COMET_TAIL) ** 2 * edges * fadeIn * (i < 3 ? 1.3 : 1);
      comet.trailColor[k] = cometColor.r * light;
      comet.trailColor[k + 1] = cometColor.g * light;
      comet.trailColor[k + 2] = cometColor.b * light;
    }
    comet.head.position.set(comet.trail[0], comet.trail[1], comet.trail[2]);
    const glowScale = comet.boost > 0 ? 1 + (boostScale - 1) * 0.8 : 1;
    material.opacity = 0.35 * edges * fadeIn * glowScale;
    comet.head.scale.set(0.35 * glowScale, 0.35 * glowScale, 1);
    comet.geometry.attributes.position.needsUpdate = true;
    comet.geometry.attributes.color.needsUpdate = true;
  }

  /** A comet hit dot `hit`: pop it, push its neighbours and start a ripple. */
  function strike(hit: number, time: number) {
    springing = true;
    rippling = true;
    const hx = dotAt[hit * 3];
    const hy = dotAt[hit * 3 + 1];
    const hz = dotAt[hit * 3 + 2];
    let slot = 0;
    let oldest = Infinity;
    for (let i = 0; i < MAX_WAVES; i++) {
      if (!waves[i].active) {
        slot = i;
        break;
      }
      if (waves[i].at < oldest) {
        oldest = waves[i].at;
        slot = i;
      }
    }
    waves[slot] = { active: true, x: hx, y: hy, z: hz, at: time, amp: HIT_FORCE };
    for (let i = 0; i < DOTS; i++) {
      const ox = dotAt[i * 3] - hx;
      const oy = dotAt[i * 3 + 1] - hy;
      const oz = dotAt[i * 3 + 2] - hz;
      const far2 = ox * ox + oy * oy + oz * oz;
      if (far2 > 4 || far2 < 1e-4) continue;
      const far = Math.sqrt(far2);
      const near = 1 - far / 2;
      const kick = (0.5 * HIT_FORCE * near * near) / Math.max(far, 0.1);
      dotVelocity[i * 3] += ox * kick;
      dotVelocity[i * 3 + 1] += oy * kick;
      dotVelocity[i * 3 + 2] += oz * kick;
      const swell = 1 + 0.8 * HIT_FORCE * near * near;
      if (swell > dotScale[i]) {
        dotScale[i] = swell;
        dotScaleVelocity[i] = 0;
      }
    }
  }

  function collide(time: number) {
    const reach = 0.8 * 0.8;
    for (const comet of comets) {
      if (!comet.racing) continue;
      const [x, y, z] = comet.trail;
      if (x === 0 && y === 0 && z === 0) continue;
      for (let i = 0; i < DOTS; i += 3) {
        const ox = dotAt[i * 3] - x;
        const oy = dotAt[i * 3 + 1] - y;
        const oz = dotAt[i * 3 + 2] - z;
        if (ox * ox + oy * oy + oz * oz < reach && dotPopAge[i] === 0) {
          dotPopAge[i] = 0.001;
          dotFlash[i] = 6 * HIT_FORCE;
          dotScale[i] = 1 + 0.3 * HIT_FORCE;
          strike(i, time);
          comet.boost = 0.4;
          comet.speed = comet.base * (1 + 0.6 * HIT_FORCE);
        }
      }
    }
  }

  function resize() {
    const width = canvas.clientWidth || 1;
    const height = canvas.clientHeight || 1;
    renderer.setSize(width, height, false);
    camera.aspect = width / height;
    camera.updateProjectionMatrix();
    if (still) renderer.render(scene, camera);
  }
  const observer = new ResizeObserver(resize);
  observer.observe(canvas);


  let frame = 0;
  let started = 0;
  let last = performance.now();
  let spinBase = 0;
  let sinceStrands = STRAND_STEP;
  let tick = 0;

  function step(now: number, dtOverride?: number) {
    const dt = dtOverride ?? Math.min((now - last) / 1000, 0.04);
    last = now;
    if (started === 0) started = now;
    const time = (now - started) / 1000;
    const grow = easeIn(time, 0, 2);
    const dotsIn = easeIn(time, 1.2, 3);
    const cometsIn = easeIn(time, 3, 5);
    spinBase += dt * FLOW_SPEED;

    sinceStrands += dt;
    if (sinceStrands >= STRAND_STEP) {
      sinceStrands -= STRAND_STEP;
      strands.forEach((strand, i) => drawStrand(strand, i, time, spinBase, grow));
      strandGeometry.attributes.position.needsUpdate = true;
      strandGeometry.attributes.color.needsUpdate = true;
    }
    if (springing) settle(dt);
    drawDots(dt, time, spinBase, dotsIn);
    for (const comet of comets) moveComet(comet, time, dt, spinBase, cometsIn);
    if (++tick % 2 === 0) collide(time);
    renderer.render(scene, camera);
  }

  resize();
  if (still) {
    // No motion: draw the finished vortex once, without comets.
    started = performance.now() - 10_000;
    step(performance.now(), 0);
    for (const comet of comets) comet.head.material.opacity = 0;
    renderer.render(scene, camera);
  } else {
    const loop = (now: number) => {
      frame = requestAnimationFrame(loop);
      step(now);
    };
    frame = requestAnimationFrame(loop);
  }

  return () => {
    cancelAnimationFrame(frame);
    observer.disconnect();
    for (const thing of disposables) thing.dispose();
    renderer.dispose();
    renderer.forceContextLoss();
  };
}

/** A small soft round glow for the comet heads. */
function radialTexture(): THREE.CanvasTexture {
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 32;
  const context = canvas.getContext("2d");
  if (context) {
    const gradient = context.createRadialGradient(16, 16, 0, 16, 16, 16);
    gradient.addColorStop(0, "rgba(255,255,255,0.9)");
    gradient.addColorStop(0.3, "rgba(255,120,255,0.4)");
    gradient.addColorStop(0.7, "rgba(200,50,200,0.08)");
    gradient.addColorStop(1, "rgba(0,0,0,0)");
    context.fillStyle = gradient;
    context.fillRect(0, 0, 32, 32);
  }
  const texture = new THREE.CanvasTexture(canvas);
  texture.needsUpdate = true;
  return texture;
}
