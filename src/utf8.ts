/**
 * Installs a UTF-8 fallback if `TextDecoder` or `TextEncoder` is undefined.
 *
 * `AudioWorkletGlobalScope` omits the WHATWG text classes in browsers. The
 * generated Wasm glue constructs `TextDecoder` and `TextEncoder` when the
 * module loads. Import this file before `pkg/tunelet.js`.
 */

interface Utf8DecoderOptions {
  fatal?: boolean;
  ignoreBOM?: boolean;
}

interface EncodeIntoResult {
  read: number;
  written: number;
}

/** Checks if `byte` is a valid UTF-8 continuation byte (`10xxxxxx`). */
function isContinuation(byte: number): boolean {
  return (byte & 0xc0) === 0x80;
}

/** Checks if `b1` is a valid first continuation byte after 3-byte `b0`. */
function isValidThreeByteFirst(b0: number, b1: number): boolean {
  if (b0 === 0xe0) {
    return b1 >= 0xa0 && b1 <= 0xbf;
  }
  if (b0 === 0xed) {
    return b1 >= 0x80 && b1 <= 0x9f;
  }
  return isContinuation(b1);
}

/** Checks if `b1` is a valid first continuation byte after 4-byte `b0`. */
function isValidFourByteFirst(b0: number, b1: number): boolean {
  if (b0 === 0xf0) {
    return b1 >= 0x90 && b1 <= 0xbf;
  }
  if (b0 === 0xf4) {
    return b1 >= 0x80 && b1 <= 0x8f;
  }
  return isContinuation(b1);
}

/** Minimal UTF-8 `TextDecoder` for scopes that omit it. */
class FallbackTextDecoder {
  readonly encoding = "utf-8";
  readonly fatal: boolean;
  readonly ignoreBOM: boolean;

  constructor(label?: string, options?: Utf8DecoderOptions) {
    if (label !== undefined) {
      const normalized = String(label).trim().toLowerCase();
      if (
        normalized !== "utf-8" &&
        normalized !== "utf8" &&
        normalized !== "unicode-1-1-utf-8"
      ) {
        throw new RangeError("The label must select UTF-8");
      }
    }
    this.fatal = Boolean(options?.fatal);
    this.ignoreBOM = Boolean(options?.ignoreBOM);
  }

  decode(input?: AllowSharedBufferSource): string {
    if (input === undefined) {
      return "";
    }
    const bytes = ArrayBuffer.isView(input)
      ? new Uint8Array(input.buffer, input.byteOffset, input.byteLength)
      : new Uint8Array(input);

    let index = 0;
    if (
      !this.ignoreBOM &&
      bytes.length >= 3 &&
      bytes[0] === 0xef &&
      bytes[1] === 0xbb &&
      bytes[2] === 0xbf
    ) {
      index = 3;
    }

    const parts: string[] = [];
    const units: number[] = [];

    while (index < bytes.length) {
      const b0 = bytes[index];
      if (b0 < 0x80) {
        units.push(b0);
        index += 1;
      } else if (b0 >= 0xc2 && b0 <= 0xdf) {
        if (index + 1 < bytes.length && isContinuation(bytes[index + 1])) {
          units.push(((b0 & 0x1f) << 6) | (bytes[index + 1] & 0x3f));
          index += 2;
        } else {
          this.failOrReplace(units);
          index += 1;
        }
      } else if (b0 >= 0xe0 && b0 <= 0xef) {
        const hasB1 =
          index + 1 < bytes.length &&
          isValidThreeByteFirst(b0, bytes[index + 1]);
        const hasB2 =
          hasB1 && index + 2 < bytes.length && isContinuation(bytes[index + 2]);
        if (hasB2) {
          units.push(
            ((b0 & 0x0f) << 12) |
              ((bytes[index + 1] & 0x3f) << 6) |
              (bytes[index + 2] & 0x3f),
          );
          index += 3;
        } else {
          this.failOrReplace(units);
          index += hasB1 ? 2 : 1;
        }
      } else if (b0 >= 0xf0 && b0 <= 0xf4) {
        const hasB1 =
          index + 1 < bytes.length &&
          isValidFourByteFirst(b0, bytes[index + 1]);
        const hasB2 =
          hasB1 && index + 2 < bytes.length && isContinuation(bytes[index + 2]);
        const hasB3 =
          hasB2 && index + 3 < bytes.length && isContinuation(bytes[index + 3]);
        if (hasB3) {
          const point =
            (((b0 & 0x07) << 18) |
              ((bytes[index + 1] & 0x3f) << 12) |
              ((bytes[index + 2] & 0x3f) << 6) |
              (bytes[index + 3] & 0x3f)) -
            0x10000;
          units.push(0xd800 | (point >> 10), 0xdc00 | (point & 0x3ff));
          index += 4;
        } else {
          this.failOrReplace(units);
          index += hasB2 ? 3 : hasB1 ? 2 : 1;
        }
      } else {
        this.failOrReplace(units);
        index += 1;
      }

      if (units.length >= 4096) {
        parts.push(String.fromCharCode(...units));
        units.length = 0;
      }
    }

    if (units.length > 0) {
      parts.push(String.fromCharCode(...units));
    }
    return parts.join("");
  }

  private failOrReplace(units: number[]): void {
    if (this.fatal) {
      throw new TypeError("Invalid UTF-8 sequence");
    }
    units.push(0xfffd);
  }
}

/** Minimal UTF-8 `TextEncoder` for scopes that omit it. */
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
    return Uint8Array.from(bytes);
  }

  encodeInto(source: string, destination: Uint8Array): EncodeIntoResult {
    const text = String(source);
    let read = 0;
    let written = 0;
    while (read < text.length) {
      const code = text.charCodeAt(read);
      if (code < 0x80) {
        if (written + 1 > destination.length) {
          break;
        }
        destination[written] = code;
        written += 1;
        read += 1;
      } else if (code < 0x800) {
        if (written + 2 > destination.length) {
          break;
        }
        destination[written] = 0xc0 | (code >> 6);
        destination[written + 1] = 0x80 | (code & 0x3f);
        written += 2;
        read += 1;
      } else if (code >= 0xd800 && code <= 0xdbff) {
        const next = read + 1 < text.length ? text.charCodeAt(read + 1) : 0;
        if (next >= 0xdc00 && next <= 0xdfff) {
          if (written + 4 > destination.length) {
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
          if (written + 3 > destination.length) {
            break;
          }
          destination[written] = 0xef;
          destination[written + 1] = 0xbf;
          destination[written + 2] = 0xbd;
          written += 3;
          read += 1;
        }
      } else if (code >= 0xdc00 && code <= 0xdfff) {
        if (written + 3 > destination.length) {
          break;
        }
        destination[written] = 0xef;
        destination[written + 1] = 0xbf;
        destination[written + 2] = 0xbd;
        written += 3;
        read += 1;
      } else {
        if (written + 3 > destination.length) {
          break;
        }
        destination[written] = 0xe0 | (code >> 12);
        destination[written + 1] = 0x80 | ((code >> 6) & 0x3f);
        destination[written + 2] = 0x80 | (code & 0x3f);
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
