/**
 * Installs UTF-8 `TextDecoder` and `TextEncoder` in `AudioWorkletGlobalScope`
 * before the generated `wasm-bindgen` module runs.
 *
 * Browsers expose `TextDecoder` and `TextEncoder` on `Window` and `Worker`,
 * but omit them from `AudioWorkletGlobalScope`. If the host already defines
 * them, this module does nothing.
 */

interface TextCodecGlobal {
  TextDecoder?: unknown;
  TextEncoder?: unknown;
}

class WorkletTextDecoder {
  readonly encoding = "utf-8";
  readonly fatal: boolean;
  readonly ignoreBOM: boolean;

  constructor(
    label = "utf-8",
    options?: { fatal?: boolean; ignoreBOM?: boolean },
  ) {
    const normalized = String(label).trim().toLowerCase();
    if (
      normalized !== "utf-8" &&
      normalized !== "utf8" &&
      normalized !== "unicode-1-1-utf-8"
    ) {
      throw new RangeError(`Unsupported encoding label: ${label}`);
    }
    this.fatal = Boolean(options?.fatal);
    this.ignoreBOM = Boolean(options?.ignoreBOM);
  }

  decode(input?: BufferSource): string {
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

    let output = "";
    const units: number[] = [];
    const flush = (): void => {
      if (units.length > 0) {
        output += String.fromCharCode(...units);
        units.length = 0;
      }
    };
    const invalid = (): void => {
      if (this.fatal) {
        throw new TypeError("Invalid UTF-8 byte sequence");
      }
      units.push(0xfffd);
    };

    while (index < bytes.length) {
      const b0 = bytes[index++];
      if (b0 < 0x80) {
        units.push(b0);
      } else if (b0 >= 0xc2 && b0 <= 0xdf) {
        if (index >= bytes.length) {
          invalid();
          break;
        }
        const b1 = bytes[index];
        if ((b1 & 0xc0) !== 0x80) {
          invalid();
        } else {
          index++;
          units.push(((b0 & 0x1f) << 6) | (b1 & 0x3f));
        }
      } else if (b0 >= 0xe0 && b0 <= 0xef) {
        if (index + 1 >= bytes.length) {
          invalid();
          break;
        }
        const b1 = bytes[index];
        const b2 = bytes[index + 1];
        const badLead =
          (b0 === 0xe0 && b1 < 0xa0) || (b0 === 0xed && b1 >= 0xa0);
        if (badLead || (b1 & 0xc0) !== 0x80 || (b2 & 0xc0) !== 0x80) {
          invalid();
        } else {
          index += 2;
          units.push(((b0 & 0x0f) << 12) | ((b1 & 0x3f) << 6) | (b2 & 0x3f));
        }
      } else if (b0 >= 0xf0 && b0 <= 0xf4) {
        if (index + 2 >= bytes.length) {
          invalid();
          break;
        }
        const b1 = bytes[index];
        const b2 = bytes[index + 1];
        const b3 = bytes[index + 2];
        const bad4 =
          (b0 === 0xf0 && b1 < 0x90) || (b0 === 0xf4 && b1 >= 0x90);
        if (
          bad4 ||
          (b1 & 0xc0) !== 0x80 ||
          (b2 & 0xc0) !== 0x80 ||
          (b3 & 0xc0) !== 0x80
        ) {
          invalid();
        } else {
          index += 3;
          const cp =
            (((b0 & 0x07) << 18) |
              ((b1 & 0x3f) << 12) |
              ((b2 & 0x3f) << 6) |
              (b3 & 0x3f)) -
            0x10000;
          units.push(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff));
        }
      } else {
        invalid();
      }
      if (units.length >= 4096) {
        flush();
      }
    }
    flush();
    return output;
  }
}

class WorkletTextEncoder {
  readonly encoding = "utf-8";

  encode(input = ""): Uint8Array {
    const out: number[] = [];
    for (let i = 0; i < input.length; i++) {
      const code = input.charCodeAt(i);
      if (code < 0x80) {
        out.push(code);
      } else if (code < 0x800) {
        out.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f));
      } else if (code >= 0xd800 && code <= 0xdbff && i + 1 < input.length) {
        const next = input.charCodeAt(i + 1);
        if (next >= 0xdc00 && next <= 0xdfff) {
          const cp = ((code - 0xd800) << 10) + (next - 0xdc00) + 0x10000;
          out.push(
            0xf0 | (cp >> 18),
            0x80 | ((cp >> 12) & 0x3f),
            0x80 | ((cp >> 6) & 0x3f),
            0x80 | (cp & 0x3f),
          );
          i++;
        } else {
          out.push(0xef, 0xbf, 0xbd);
        }
      } else if (code >= 0xd800 && code <= 0xdfff) {
        out.push(0xef, 0xbf, 0xbd);
      } else {
        out.push(
          0xe0 | (code >> 12),
          0x80 | ((code >> 6) & 0x3f),
          0x80 | (code & 0x3f),
        );
      }
    }
    return new Uint8Array(out);
  }

  encodeInto(
    source: string,
    destination: Uint8Array,
  ): { read: number; written: number } {
    let read = 0;
    let written = 0;
    while (read < source.length) {
      const code = source.charCodeAt(read);
      if (code < 0x80) {
        if (written + 1 > destination.length) break;
        destination[written++] = code;
        read += 1;
      } else if (code < 0x800) {
        if (written + 2 > destination.length) break;
        destination[written++] = 0xc0 | (code >> 6);
        destination[written++] = 0x80 | (code & 0x3f);
        read += 1;
      } else if (code >= 0xd800 && code <= 0xdbff && read + 1 < source.length) {
        const next = source.charCodeAt(read + 1);
        if (next >= 0xdc00 && next <= 0xdfff) {
          if (written + 4 > destination.length) break;
          const cp = ((code - 0xd800) << 10) + (next - 0xdc00) + 0x10000;
          destination[written++] = 0xf0 | (cp >> 18);
          destination[written++] = 0x80 | ((cp >> 12) & 0x3f);
          destination[written++] = 0x80 | ((cp >> 6) & 0x3f);
          destination[written++] = 0x80 | (cp & 0x3f);
          read += 2;
        } else {
          if (written + 3 > destination.length) break;
          destination[written++] = 0xef;
          destination[written++] = 0xbf;
          destination[written++] = 0xbd;
          read += 1;
        }
      } else {
        if (written + 3 > destination.length) break;
        const val = code >= 0xd800 && code <= 0xdfff ? 0xfffd : code;
        destination[written++] = 0xe0 | (val >> 12);
        destination[written++] = 0x80 | ((val >> 6) & 0x3f);
        destination[written++] = 0x80 | (val & 0x3f);
        read += 1;
      }
    }
    return { read, written };
  }
}

const codecGlobal = globalThis as unknown as TextCodecGlobal;
if (typeof codecGlobal.TextDecoder === "undefined") {
  codecGlobal.TextDecoder = WorkletTextDecoder;
}
if (typeof codecGlobal.TextEncoder === "undefined") {
  codecGlobal.TextEncoder = WorkletTextEncoder;
}

export {};
