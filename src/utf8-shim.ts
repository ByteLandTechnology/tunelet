/**
 * Internal UTF-8 `TextDecoder` and `TextEncoder` shim for `AudioWorkletGlobalScope`.
 *
 * Standard `AudioWorkletGlobalScope` environments do not expose `TextDecoder`
 * or `TextEncoder`. The generated wasm-bindgen module constructs both at
 * evaluation time. If `globalThis` does not define them, this module installs
 * minimal UTF-8 classes.
 */

/** The options for the UTF-8 decoder constructor. */
export interface Utf8DecoderOptions {
  readonly fatal?: boolean;
  readonly ignoreBOM?: boolean;
}

/** The counts from a bounded UTF-8 encode into a target `Uint8Array`. */
export interface Utf8EncodeIntoResult {
  read: number;
  written: number;
}

const CHUNK_UNITS = 4096;

/** A UTF-8 `TextDecoder` for worklet scopes that lack a native decoder. */
class Utf8TextDecoder {
  readonly encoding = "utf-8";
  readonly fatal: boolean;
  readonly ignoreBOM: boolean;

  constructor(label = "utf-8", options?: Utf8DecoderOptions) {
    const normalized = String(label).trim().toLowerCase();
    if (normalized !== "utf-8" && normalized !== "utf8") {
      throw new RangeError(`The encoding label "${label}" is not supported.`);
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
    const length = bytes.length;
    if (
      !this.ignoreBOM &&
      length >= 3 &&
      bytes[0] === 0xef &&
      bytes[1] === 0xbb &&
      bytes[2] === 0xbf
    ) {
      index = 3;
    }

    const units: number[] = [];
    let result = "";

    const fail = (): void => {
      if (this.fatal) {
        throw new TypeError("The byte sequence is not valid UTF-8.");
      }
      units.push(0xfffd);
    };

    while (index < length) {
      const b0 = bytes[index];
      if (b0 < 0x80) {
        units.push(b0);
        index += 1;
      } else if (b0 >= 0xc2 && b0 <= 0xdf) {
        if (index + 1 >= length) {
          fail();
          break;
        }
        const b1 = bytes[index + 1];
        if ((b1 & 0xc0) !== 0x80) {
          fail();
          index += 1;
        } else {
          units.push(((b0 & 0x1f) << 6) | (b1 & 0x3f));
          index += 2;
        }
      } else if (b0 >= 0xe0 && b0 <= 0xef) {
        if (index + 2 >= length) {
          fail();
          break;
        }
        const b1 = bytes[index + 1];
        const b2 = bytes[index + 2];
        const validSecond =
          (b0 === 0xe0 ? b1 >= 0xa0 && b1 <= 0xbf : true) &&
          (b0 === 0xed ? b1 >= 0x80 && b1 <= 0x9f : true) &&
          (b1 & 0xc0) === 0x80;
        if (!validSecond || (b2 & 0xc0) !== 0x80) {
          fail();
          index += 1;
        } else {
          units.push(((b0 & 0x0f) << 12) | ((b1 & 0x3f) << 6) | (b2 & 0x3f));
          index += 3;
        }
      } else if (b0 >= 0xf0 && b0 <= 0xf4) {
        if (index + 3 >= length) {
          fail();
          break;
        }
        const b1 = bytes[index + 1];
        const b2 = bytes[index + 2];
        const b3 = bytes[index + 3];
        const validSecond =
          (b0 === 0xf0 ? b1 >= 0x90 && b1 <= 0xbf : true) &&
          (b0 === 0xf4 ? b1 >= 0x80 && b1 <= 0x8f : true) &&
          (b1 & 0xc0) === 0x80;
        if (
          !validSecond ||
          (b2 & 0xc0) !== 0x80 ||
          (b3 & 0xc0) !== 0x80
        ) {
          fail();
          index += 1;
        } else {
          const codePoint =
            (((b0 & 0x07) << 18) |
              ((b1 & 0x3f) << 12) |
              ((b2 & 0x3f) << 6) |
              (b3 & 0x3f)) -
            0x10000;
          units.push(0xd800 | (codePoint >> 10), 0xdc00 | (codePoint & 0x3ff));
          index += 4;
        }
      } else {
        fail();
        index += 1;
      }

      if (units.length >= CHUNK_UNITS) {
        result += String.fromCharCode(...units);
        units.length = 0;
      }
    }

    if (units.length > 0) {
      result += String.fromCharCode(...units);
    }
    return result;
  }
}

/** A UTF-8 `TextEncoder` for worklet scopes that lack a native encoder. */
class Utf8TextEncoder {
  readonly encoding = "utf-8";

  encode(input = ""): Uint8Array {
    const text = String(input);
    const bytes: number[] = [];
    for (let index = 0; index < text.length; index += 1) {
      let code = text.charCodeAt(index);
      if (code < 0x80) {
        bytes.push(code);
      } else if (code < 0x800) {
        bytes.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f));
      } else if (code >= 0xd800 && code <= 0xdbff) {
        const next = index + 1 < text.length ? text.charCodeAt(index + 1) : 0;
        if (next >= 0xdc00 && next <= 0xdfff) {
          code = 0x10000 + (((code & 0x3ff) << 10) | (next & 0x3ff));
          bytes.push(
            0xf0 | (code >> 18),
            0x80 | ((code >> 12) & 0x3f),
            0x80 | ((code >> 6) & 0x3f),
            0x80 | (code & 0x3f),
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

  encodeInto(source: string, destination: Uint8Array): Utf8EncodeIntoResult {
    const text = String(source);
    const limit = destination.length;
    let read = 0;
    let written = 0;

    while (read < text.length) {
      const code = text.charCodeAt(read);
      if (code < 0x80) {
        if (written + 1 > limit) break;
        destination[written] = code;
        written += 1;
        read += 1;
      } else if (code < 0x800) {
        if (written + 2 > limit) break;
        destination[written] = 0xc0 | (code >> 6);
        destination[written + 1] = 0x80 | (code & 0x3f);
        written += 2;
        read += 1;
      } else if (code >= 0xd800 && code <= 0xdbff) {
        const next = read + 1 < text.length ? text.charCodeAt(read + 1) : 0;
        if (next >= 0xdc00 && next <= 0xdfff) {
          if (written + 4 > limit) break;
          const point = 0x10000 + (((code & 0x3ff) << 10) | (next & 0x3ff));
          destination[written] = 0xf0 | (point >> 18);
          destination[written + 1] = 0x80 | ((point >> 12) & 0x3f);
          destination[written + 2] = 0x80 | ((point >> 6) & 0x3f);
          destination[written + 3] = 0x80 | (point & 0x3f);
          written += 4;
          read += 2;
        } else {
          if (written + 3 > limit) break;
          destination[written] = 0xef;
          destination[written + 1] = 0xbf;
          destination[written + 2] = 0xbd;
          written += 3;
          read += 1;
        }
      } else {
        if (written + 3 > limit) break;
        const unit = code >= 0xdc00 && code <= 0xdfff ? 0xfffd : code;
        destination[written] = 0xe0 | (unit >> 12);
        destination[written + 1] = 0x80 | ((unit >> 6) & 0x3f);
        destination[written + 2] = 0x80 | (unit & 0x3f);
        written += 3;
        read += 1;
      }
    }

    return { read, written };
  }
}

/** If `globalThis` lacks `TextDecoder` or `TextEncoder`, installs the UTF-8 shim. */
export function installUtf8Shim(): void {
  const scope = globalThis as Record<string, unknown>;
  if (typeof scope.TextDecoder === "undefined") {
    scope.TextDecoder = Utf8TextDecoder;
  }
  if (typeof scope.TextEncoder === "undefined") {
    scope.TextEncoder = Utf8TextEncoder;
  }
}

installUtf8Shim();
