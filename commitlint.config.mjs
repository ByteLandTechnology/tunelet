// Commit message rules for Conventional Commits and AGENTS.md.

const HEADER_PATTERN = /^[a-z]+(?:\([a-z0-9._/-]+\))?!?: \S.*$/;
const ATTRIBUTION_PATTERN = /\b(?:co-authored-by|assisted-by)\s*:/i;
const SESSION_LABEL_PATTERN = /\bsession(?:\s*[- ]\s*(?:link|url|id))?\s*:/i;
const SESSION_URL_PATTERN =
  /\bhttps?:\/\/\S*(?:session|chat|thread|conversation|claude\.ai|chatgpt\.com|jules\.google|stitch\.withgoogle|aistudio\.google|cursor\.com|ampcode\.com|devin\.ai|copilot)\S*/i;

/** Checks whether the CLI reads a message file with `--edit` or `--env`. */
function isEditMode() {
  const args = process.argv.slice(2);
  return args.some(
    (arg) =>
      arg === "-e" ||
      arg === "--edit" ||
      arg.startsWith("--edit=") ||
      arg === "-E" ||
      arg === "--env" ||
      arg.startsWith("--env="),
  );
}

/** Checks whether the CLI reads commits from a Git history range. */
function isHistoryRangeMode() {
  const args = process.argv.slice(2);
  return args.some(
    (arg) =>
      arg === "-f" ||
      arg === "--from" ||
      arg.startsWith("--from=") ||
      arg === "-t" ||
      arg === "--to" ||
      arg.startsWith("--to=") ||
      arg === "--from-last-tag",
  );
}

/** Normalizes line breaks and removes wrapper text from commitlint or Git. */
function extractUserMessage(raw) {
  let text = (raw ?? "").replace(/\r\n/g, "\n");

  // `@commitlint/read` adds one newline at the end in edit and range modes.
  if ((isEditMode() || isHistoryRangeMode()) && text.endsWith("\n")) {
    text = text.slice(0, -1);
  }

  if (isEditMode()) {
    // Remove the Git comment block at the end and any other `#` comment lines.
    text = text.replace(/\n(?:#.*(?:\n|$))+$/, "");
    text = text
      .split("\n")
      .filter((line) => !line.startsWith("#"))
      .join("\n");
  }

  return text;
}

/** Splits the user message into lines after it removes one final newline. */
function getMessageLines(raw) {
  const text = extractUserMessage(raw);
  const withoutFinalNewline = text.endsWith("\n") ? text.slice(0, -1) : text;
  return withoutFinalNewline.split("\n");
}

const tuneletPlugin = {
  rules: {
    "header-syntax": (parsed) => {
      const [header = ""] = getMessageLines(parsed.raw);
      return [
        HEADER_PATTERN.test(header),
        "Header must match 'type(scope): summary' or 'type: summary'.",
      ];
    },
    "line-max-columns": (parsed, _when, maxColumns = 72) => {
      const lines = getMessageLines(parsed.raw);
      const tooLong = lines.some((line) => line.length > maxColumns);
      return [
        !tooLong,
        `Keep each line at or below ${maxColumns} columns.`,
      ];
    },
    "body-bullet-list": (parsed) => {
      const lines = getMessageLines(parsed.raw);
      if (lines.length <= 1) {
        return [true];
      }
      if (lines[1] !== "") {
        return [
          false,
          "Put one blank line between the header and the body.",
        ];
      }
      const bodyLines = lines.slice(2).filter((line) => line.trim() !== "");
      for (const line of bodyLines) {
        if (!line.startsWith("- ") || line.length <= 2 || /^- \s/.test(line)) {
          return [false, "Start each body line with '- '."];
        }
        if (!line.endsWith(".") || line === "- .") {
          return [false, "End each '- ' body bullet with a period."];
        }
      }
      return [true];
    },
    "no-literal-newlines": (parsed) => {
      const text = extractUserMessage(parsed.raw);
      return [
        !text.includes("\\n"),
        "Do not use literal '\\n' text in a commit message.",
      ];
    },
    "no-extra-blank-lines": (parsed) => {
      const text = extractUserMessage(parsed.raw);
      if (text.startsWith("\n")) {
        return [false, "Do not put blank lines before the header."];
      }
      if (text.endsWith("\n\n")) {
        return [
          false,
          "Do not put extra blank lines at the end of the message.",
        ];
      }
      const lines = getMessageLines(parsed.raw);
      if (lines.length === 2 && lines[1] === "") {
        return [
          false,
          "Do not put extra blank lines at the end of the message.",
        ];
      }
      if (lines.length >= 3 && lines.slice(2).some((line) => line.trim() === "")) {
        return [
          false,
          "Do not put blank lines inside or after the body list.",
        ];
      }
      return [true];
    },
    "no-attribution": (parsed) => {
      const text = extractUserMessage(parsed.raw);
      return [
        !ATTRIBUTION_PATTERN.test(text),
        "Do not add Co-Authored-By or Assisted-by trailers.",
      ];
    },
    "no-session-links": (parsed) => {
      const text = extractUserMessage(parsed.raw);
      const hasSessionLink =
        SESSION_LABEL_PATTERN.test(text) || SESSION_URL_PATTERN.test(text);
      return [!hasSessionLink, "Do not add session links to commit messages."];
    },
  },
};

export default {
  extends: ["@commitlint/config-conventional"],
  plugins: [tuneletPlugin],
  rules: {
    "header-max-length": [2, "always", 72],
    "body-leading-blank": [2, "always"],
    "body-max-line-length": [2, "always", 72],
    "footer-leading-blank": [2, "always"],
    "footer-max-line-length": [2, "always", 72],
    "scope-case": [2, "always", "lower-case"],
    "header-syntax": [2, "always"],
    "line-max-columns": [2, "always", 72],
    "body-bullet-list": [2, "always"],
    "no-literal-newlines": [2, "always"],
    "no-extra-blank-lines": [2, "always"],
    "no-attribution": [2, "always"],
    "no-session-links": [2, "always"],
  },
};
