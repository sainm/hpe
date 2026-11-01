import { loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor/editor/editor.api";
import "monaco-editor/language/json/monaco.contribution";
import EditorWorker from "monaco-editor/editor/editor.worker.js?worker";
import JsonWorker from "monaco-editor/language/json/json.worker.js?worker";

const workerScope = self as unknown as {
  MonacoEnvironment: {
    getWorker: (moduleId: string, label: string) => Worker;
  };
};

workerScope.MonacoEnvironment = {
  getWorker: (_moduleId, label) => {
    if (label === "json") {
      return new JsonWorker();
    }
    return new EditorWorker();
  },
};

loader.config({ monaco });

if (!monaco.languages.getLanguages().some((language) => language.id === "hpa-yaml")) {
  monaco.languages.register({ id: "hpa-yaml" });
  monaco.languages.setLanguageConfiguration("hpa-yaml", {
    comments: { lineComment: "#" },
    brackets: [
      ["{", "}"],
      ["[", "]"],
    ],
    autoClosingPairs: [
      { open: "{", close: "}" },
      { open: "[", close: "]" },
      { open: '"', close: '"' },
    ],
  });
  monaco.languages.setMonarchTokensProvider("hpa-yaml", {
    tokenizer: {
      root: [
        [/^\s*#.*/, "comment"],
        [/^\s*-\s+/, "keyword"],
        [/^\s*[\w.$/ -]+(?=\s*:)/, "type.identifier"],
        [/\$\{[^}]*\}/, "variable"],
        [/\$expr|\$template/, "keyword"],
        [/\b(true|false|null)\b/, "keyword"],
        [/-?\d+(\.\d+)?/, "number"],
        [/"/, { token: "string", next: "@string" }],
        [/'/, { token: "string", next: "@single" }],
      ],
      string: [
        [/[^"\\]+/, "string"],
        [/\\./, "string.escape"],
        [/"/, { token: "string", next: "@pop" }],
      ],
      single: [
        [/[^'\\]+/, "string"],
        [/\\./, "string.escape"],
        [/'/, { token: "string", next: "@pop" }],
      ],
    },
  });
}

monaco.editor.defineTheme("hpa-dark", {
  base: "vs-dark",
  inherit: true,
  rules: [
    { token: "type.identifier", foreground: "6ba5e8" },
    { token: "variable", foreground: "8fb6f0" },
    { token: "keyword", foreground: "b79be0" },
  ],
  colors: {
    "editor.background": "#1b1e22",
    "editorGutter.background": "#1b1e22",
    "editor.lineHighlightBackground": "#202429",
    "editorLineNumber.foreground": "#59626d",
    "editorLineNumber.activeForeground": "#a7b0ba",
    "editor.selectionBackground": "#1b2939",
  },
});

monaco.editor.defineTheme("hpa-light", {
  base: "vs",
  inherit: true,
  rules: [
    { token: "type.identifier", foreground: "1557b0" },
    { token: "variable", foreground: "12499e" },
    { token: "keyword", foreground: "5b2b8c" },
  ],
  colors: {
    "editor.background": "#fafbfc",
    "editorGutter.background": "#fafbfc",
    "editor.lineHighlightBackground": "#edeff2",
    "editorLineNumber.foreground": "#a8b0ba",
    "editorLineNumber.activeForeground": "#79838f",
    "editor.selectionBackground": "#e9f0fa",
  },
});
