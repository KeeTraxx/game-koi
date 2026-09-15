// The audio side of the pacing clock. This runs on the browser's audio thread, woken
// on a schedule the sound hardware drives rather than the display. It owns a ring
// buffer that the main thread fills; all it does is drain it and report how full it
// is.
//
// Why a ring and not a message per frame: postMessage allocates and is queued behind
// whatever else the audio thread has to do, and this callback has a hard deadline. A
// pre-allocated ring written by one thread and read by another needs no allocation and
// no lock for a single-producer/single-consumer pair like this one.
//
// Kept as a source string, not a separate .js file, so `GameKoi` can register it via a
// Blob URL — a consumer never needs to serve this file or point a bundler at it.
export const WORKLET_SOURCE = `
  const CAPACITY = 48000; // ~0.5s at 48kHz stereo: rides out a slow main thread.

  class KoiProcessor extends AudioWorkletProcessor {
    constructor() {
      super();
      this.left = new Float32Array(CAPACITY);
      this.right = new Float32Array(CAPACITY);
      this.readIndex = 0;
      this.writeIndex = 0;

      this.port.onmessage = (event) => {
        const { left, right } = event.data;
        for (let i = 0; i < left.length; i++) {
          const next = (this.writeIndex + 1) % CAPACITY;
          // Full: drop the oldest rather than the newest, so audio does not fall
          // permanently further behind the picture.
          if (next === this.readIndex) {
            this.readIndex = (this.readIndex + 1) % CAPACITY;
          }
          this.left[this.writeIndex] = left[i];
          this.right[this.writeIndex] = right[i];
          this.writeIndex = next;
        }
      };
    }

    available() {
      return (this.writeIndex - this.readIndex + CAPACITY) % CAPACITY;
    }

    process(_inputs, outputs) {
      const output = outputs[0];
      const outL = output[0];
      const outR = output.length > 1 ? output[1] : output[0];

      for (let i = 0; i < outL.length; i++) {
        if (this.readIndex === this.writeIndex) {
          // Silence on underrun. Holding the last sample turns a click into a buzz,
          // which is worse.
          outL[i] = 0;
          outR[i] = 0;
        } else {
          outL[i] = this.left[this.readIndex];
          outR[i] = this.right[this.readIndex];
          this.readIndex = (this.readIndex + 1) % CAPACITY;
        }
      }

      // The main thread paces itself off this number, so it goes back every block.
      this.port.postMessage({ buffered: this.available() });
      return true;
    }
  }

  registerProcessor("koi-processor", KoiProcessor);
`;
