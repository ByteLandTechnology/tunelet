/**
 * Minimal UTF-8 `TextDecoder` and `TextEncoder` shims for `AudioWorkletGlobalScope`.
 *
 * Some browser `AudioWorkletGlobalScope` runtimes omit `TextDecoder` and
 * `TextEncoder`. The generated wasm-bindgen module constructs both classes at
 * module load time. If the global scope already defines a class, this module
 * keeps that native class.
 */

interface MinimalTextDecoderOptions {
  fatal?: boolean;
  ignoreBOM?: boolean;
}

interface MinimalTextDecodeOptions {
  stream?: boolean;
}

interface MinimalEncodeIntoResult {
  read: number;
  written: number;
}

function toBytes(input?: AllowSharedBufferSource): Uint8Array {
  if (input === undefined) {
    return new Uint8Array(0);
  }
  if (input instanceof Uint8Array) {
    return input;
  }
  if (ArrayBuffer.isView(input)) {
    return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
  }
  return new Uint8Array(input);
}

class FallbackTextDecoder {
  readonly encoding = "utf-8";
  readonly fatal: boolean;
  readonly ignoreBOM: boolean;

  constructor(label = "utf-8", options?: MinimalTextDecoderOptions) {
    const normalized = String(label).trim().toLowerCase();
    if (normalized !== "utf-8" && normalized !== "utf8") {
      throw new RangeError(`TextDecoder does not support label: ${label}`);
    }
    this.fatal = Boolean(options?.fatal);
    this.ignoreBOM = Boolean(options?.ignoreBOM);
  }

  decode(
    input?: AllowSharedBufferSource,
    options?: MinimalTextDecodeOptions,
  ): string {
    if (options?.stream) {
      throw new TypeError("TextDecoder stream mode is not supported");
    }
    const bytes = toBytes(input);
    const length = bytes.length;
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
            (b1 & 0xc0) === 0x80 &&
            (b0 !== 0xe0 || b1 >= 0xa0) &&
            (b0 !== 0xed || b1 <= 0x9f);
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
            (b1 & 0xc0) === 0x80 &&
            (b0 !== 0xf0 || b1 >= 0x90) &&
            (b0 !== 0xf4 || b1 <= 0x8f);
          if (
            validSecond &&
            (b2 & 0xc0) === 0x80 &&
            (b3 & 0xc0) === 0x80
          ) {
            const codePoint =
              ((b0 & 0x07) << 18) |
              ((b1 & 0x3f) << 12) |
              ((b2 & 0x3f) << 6) |
              (b3 & 0x3f);
            const offset = codePoint - 0x10000;
            result += String.fromCharCode(
              0xd800 + (offset >> 10),
              0xdc00 + (offset & 0x3ff),
            );
            index += 4;
            continue;
          }
        }
      }

      if (this.fatal) {
        throw new TypeError("Invalid UTF-8 byte sequence");
      }
      result += "\ufffd";
      index += 1;
    }

    return result;
  }
}

class FallbackTextEncoder {
  readonly encoding = "utf-8";

  encode(input = ""): Uint8Array {
    const text = String(input);
    const bytes: number[] = [];
    for (let index = 0; index < text.length; index += 1) {
      const code = text.charCodeAt(index);
      if (code < 0x80) {
        bytes.push(code);
      } else if (code < 0x800) {
        bytes.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f));
      } else if (code >= 0xd800 && code <= 0xdbff) {
        const next = index + 1 < text.length ? text.charCodeAt(index + 1) : 0;
        if (next >= 0xdc00 && next <= 0xdfff) {
          const point = 0x10000 + (((code & 0x3ff) << 10) | (next & 0x3ff));
          bytes.push(
            0xf0 | (point >> 18),
            0x80 | ((point >> 12) & 0x3f),
            0x80 | ((point >> 6) & 0x3f),
            0x80 | (point & 0x3f),
          );
          index += 1;
        } else {
          bytes.push(0xef, 0xbf, 0xbd);
        }
      } else if (code >= 0xdc00 && code <= 0xdfff) {
        bytes.push(0xef, 0xbf, 0xbd);
      } else {
        bytes.push(
          0xe0 | (code >> 12),
          0x80 | ((code >> 6) & 0x3f),
          0x80 | (code & 0x3f),
        );
      }
    }
    return new Uint8Array(bytes);
  }

  encodeInto(
    source: string,
    destination: Uint8Array,
  ): MinimalEncodeIntoResult {
    let read = 0;
    let written = 0;
    const limit = destination.length;
    while (read < source.length) {
      const code = source.charCodeAt(read);
      if (code < 0x80) {
        if (written + 1 > limit) {
          break;
        }
        destination[written] = code;
        written += 1;
        read += 1;
      } else if (code < 0x800) {
        if (written + 2 > limit) {
          break;
        }
        destination[written] = 0xc0 | (code >> 6);
        destination[written + 1] = 0x80 | (code & 0x3f);
        written += 2;
        read += 1;
      } else if (code >= 0xd800 && code <= 0xdbff) {
        const next = read + 1 < source.length ? source.charCodeAt(read + 1) : 0;
        if (next >= 0xdc00 && next <= 0xdfff) {
          if (written + 4 > limit) {
            break;
          }
          const point = 0x10000 + (((code & 0x3ff) << 10) | (next & 0x3ff));
          destination[written] = 0xf0 | (point >> 18);
          destination[written + 1] = 0x80 | ((point >> 12) & 0x3f);
          destination[written + 2] = 0x80 | ((point >> 6) & 0x3f);
          destination[written + 3] = 0x80 | (point & 0x3f);
          written += 4;
          read += 2;
        } else {
          if (written + 3 > limit) {
            break;
          }
          destination[written] = 0xef;
          destination[written + 1] = 0xbf;
          destination[written + 2] = 0xbd;
          written += 3;
          read += 1;
        }
      } else {
        if (written + 3 > limit) {
          break;
        }
        const value = code >= 0xdc00 && code <= 0xdfff ? 0xfffd : code;
        destination[written] = 0xe0 | (value >> 12);
        destination[written + 1] = 0x80 | ((value >> 6) & 0x3f);
        destination[written + 2] = 0x80 | (value & 0x3f);
        written += 3;
        read += 1;
      }
    }
    return { read, written };
  }
}

const scope = globalThis as Record<string, unknown>;
if (typeof scope.TextDecoder === "undefined") {
  scope.TextDecoder = FallbackTextDecoder;
}
if (typeof scope.TextEncoder === "undefined") {
  scope.TextEncoder = FallbackTextEncoder;
}

export {};
