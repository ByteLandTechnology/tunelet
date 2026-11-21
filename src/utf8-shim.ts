/**
 * The internal UTF-8 shim for `AudioWorkletGlobalScope`.
 *
 * Browsers do not expose `TextDecoder` or `TextEncoder` on the audio thread.
 * The generated Wasm glue constructs both classes when the module loads. Import
 * this file before `../pkg/tunelet.js` so `globalThis` has both classes first.
 */

interface ShimDecoderOptions {
  fatal?: boolean;
  ignoreBOM?: boolean;
}

class Utf8TextDecoder {
  readonly encoding = "utf-8";
  readonly fatal: boolean;
  readonly ignoreBOM: boolean;

  constructor(label = "utf-8", options?: ShimDecoderOptions) {
    const normalized = String(label).trim().toLowerCase();
    if (
      normalized !== "utf-8" &&
      normalized !== "utf8" &&
      normalized !== "unicode-1-1-utf-8"
    ) {
      throw new RangeError(`The encoding label "${label}" is not supported.`);
    }
    this.fatal = Boolean(options?.fatal);
    this.ignoreBOM = Boolean(options?.ignoreBOM);
  }

  decode(input?: AllowSharedBufferSource): string {
    if (input === undefined) {
      return "";
    }
    const bytes = toUint8View(input);
    const length = bytes.length;
    if (length === 0) {
      return "";
    }

    let index = 0;
    if (
      !this.ignoreBOM &&
      length >= 3 &&
      bytes[0] === 0xef &&
      bytes[1] === 0xbb &&
      bytes[2] === 0xbf
    ) {
      index = 3;
    }

    let result = "";
    while (index < length) {
      const b0 = bytes[index];
      if (b0 < 0x80) {
        result += String.fromCharCode(b0);
        index += 1;
        continue;
      }

      if (b0 >= 0xc2 && b0 <= 0xdf) {
        if (index + 1 < length) {
          const b1 = bytes[index + 1];
          if ((b1 & 0xc0) === 0x80) {
            result += String.fromCharCode(((b0 & 0x1f) << 6) | (b1 & 0x3f));
            index += 2;
            continue;
          }
        }
      } else if (b0 >= 0xe0 && b0 <= 0xef) {
        if (index + 2 < length) {
          const b1 = bytes[index + 1];
          const b2 = bytes[index + 2];
          const validSecond =
            b0 === 0xe0
              ? b1 >= 0xa0 && b1 <= 0xbf
              : b0 === 0xed
                ? b1 >= 0x80 && b1 <= 0x9f
                : (b1 & 0xc0) === 0x80;
          if (validSecond && (b2 & 0xc0) === 0x80) {
            result += String.fromCharCode(
              ((b0 & 0x0f) << 12) | ((b1 & 0x3f) << 6) | (b2 & 0x3f),
            );
            index += 3;
            continue;
          }
        }
      } else if (b0 >= 0xf0 && b0 <= 0xf4) {
        if (index + 3 < length) {
          const b1 = bytes[index + 1];
          const b2 = bytes[index + 2];
          const b3 = bytes[index + 3];
          const validSecond =
            b0 === 0xf0
              ? b1 >= 0x90 && b1 <= 0xbf
              : b0 === 0xf4
                ? b1 >= 0x80 && b1 <= 0x8f
                : (b1 & 0xc0) === 0x80;
          if (validSecond && (b2 & 0xc0) === 0x80 && (b3 & 0xc0) === 0x80) {
            const codePoint =
              ((b0 & 0x07) << 18) |
              ((b1 & 0x3f) << 12) |
              ((b2 & 0x3f) << 6) |
              (b3 & 0x3f);
            result += String.fromCodePoint(codePoint);
            index += 4;
            continue;
          }
        }
      }

      if (this.fatal) {
        throw new TypeError("The byte sequence is not valid UTF-8.");
      }
      result += "\ufffd";
      index += 1;
    }

    return result;
  }
}

class Utf8TextEncoder {
  readonly encoding = "utf-8";

  encode(input = ""): Uint8Array {
    const text = String(input);
    const buffer = new Uint8Array(text.length * 3);
    const { written } = this.encodeInto(text, buffer);
    return buffer.slice(0, written);
  }

  encodeInto(
    source: string,
    destination: Uint8Array,
  ): { read: number; written: number } {
    const text = String(source);
    const limit = destination.length;
    let read = 0;
    let written = 0;

    while (read < text.length) {
      const first = text.charCodeAt(read);
      if (first < 0x80) {
        if (written + 1 > limit) {
          break;
        }
        destination[written] = first;
        read += 1;
        written += 1;
      } else if (first < 0x800) {
        if (written + 2 > limit) {
          break;
        }
        destination[written] = 0xc0 | (first >> 6);
        destination[written + 1] = 0x80 | (first & 0x3f);
        read += 1;
        written += 2;
      } else if (first >= 0xd800 && first <= 0xdbff) {
        if (read + 1 < text.length) {
          const second = text.charCodeAt(read + 1);
          if (second >= 0xdc00 && second <= 0xdfff) {
            if (written + 4 > limit) {
              break;
            }
            const codePoint =
              0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00);
            destination[written] = 0xf0 | (codePoint >> 18);
            destination[written + 1] = 0x80 | ((codePoint >> 12) & 0x3f);
            destination[written + 2] = 0x80 | ((codePoint >> 6) & 0x3f);
            destination[written + 3] = 0x80 | (codePoint & 0x3f);
            read += 2;
            written += 4;
            continue;
          }
        }
        if (written + 3 > limit) {
          break;
        }
        destination[written] = 0xef;
        destination[written + 1] = 0xbf;
        destination[written + 2] = 0xbd;
        read += 1;
        written += 3;
      } else if (first >= 0xdc00 && first <= 0xdfff) {
        if (written + 3 > limit) {
          break;
        }
        destination[written] = 0xef;
        destination[written + 1] = 0xbf;
        destination[written + 2] = 0xbd;
        read += 1;
        written += 3;
      } else {
        if (written + 3 > limit) {
          break;
        }
        destination[written] = 0xe0 | (first >> 12);
        destination[written + 1] = 0x80 | ((first >> 6) & 0x3f);
        destination[written + 2] = 0x80 | (first & 0x3f);
        read += 1;
        written += 3;
      }
    }

    return { read, written };
  }
}

/** Returns a `Uint8Array` view over any buffer source without a copy. */
function toUint8View(input: AllowSharedBufferSource): Uint8Array {
  if (input instanceof Uint8Array) {
    return input;
  }
  if (ArrayBuffer.isView(input)) {
    return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
  }
  return new Uint8Array(input);
}

const scope = globalThis as {
  TextDecoder?: unknown;
  TextEncoder?: unknown;
};

scope.TextDecoder ??= Utf8TextDecoder;
scope.TextEncoder ??= Utf8TextEncoder;

export {};
