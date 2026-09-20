import CodeMirror from "@uiw/react-codemirror";
import { yaml } from "@codemirror/lang-yaml";
import { EditorView } from "@codemirror/view";
import { useMemo } from "react";
import { useDarkMode } from "@/app/theme";

const editorTheme = EditorView.theme({
  "&": { fontSize: "12.5px", height: "100%" },
  ".cm-scroller": { fontFamily: "var(--font-mono)" },
  ".cm-content": { userSelect: "text" },
});

export function YamlEditor({
  value,
  onChange,
  readOnly = false,
  height = "100%",
}: {
  value: string;
  onChange?: (v: string) => void;
  readOnly?: boolean;
  height?: string;
}) {
  const dark = useDarkMode();
  const extensions = useMemo(() => [yaml(), editorTheme, EditorView.lineWrapping], []);
  return (
    <CodeMirror
      value={value}
      onChange={(v) => onChange?.(v)}
      extensions={extensions}
      theme={dark ? "dark" : "light"}
      height={height}
      readOnly={readOnly}
      editable={!readOnly}
      basicSetup={{ lineNumbers: true, foldGutter: true, highlightActiveLine: !readOnly, tabSize: 2 }}
      style={{ height, minHeight: 0, flex: 1 }}
    />
  );
}
