// Rendu Markdown des réponses de l'assistant, avec actions sur les blocs de code.
import { Copy, TerminalWindow } from "@phosphor-icons/react";
import { Children, isValidElement, type ReactElement, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { api } from "@/api/client";
import { notify, useStore } from "@/app/store";
import "./Markdown.css";

function textOf(node: ReactNode): string {
  if (typeof node === "string") return node;
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (isValidElement(node)) return textOf((node.props as { children?: ReactNode }).children);
  return "";
}

function CodeBlock({ children }: { children?: ReactNode }) {
  const child = Children.toArray(children)[0];
  const el = isValidElement(child) ? (child as ReactElement<{ className?: string; children?: ReactNode }>) : null;
  const language = /language-([\w-]+)/.exec(el?.props.className ?? "")?.[1] ?? "";
  const text = textOf(el?.props.children ?? children).replace(/\n$/, "");
  const isYaml = language === "yaml" || language === "yml" || /^(apiVersion|kind):/m.test(text);
  return (
    <div className="md-code">
      <div className="md-code-bar">
        <span className="muted xs">{language || "texte"}</span>
        <span className="grow" />
        {isYaml && (
          <button
            className="btn btn-ghost btn-sm"
            title="Ouvrir dans la console YAML"
            onClick={() => useStore.getState().openYamlConsole(text)}
          >
            <TerminalWindow size={13} /> Console YAML
          </button>
        )}
        <button
          className="btn btn-ghost btn-sm"
          title="Copier"
          onClick={() => navigator.clipboard.writeText(text).then(() => notify.ok("Copié."))}
        >
          <Copy size={13} /> Copier
        </button>
      </div>
      <pre>
        <code>{text}</code>
      </pre>
    </div>
  );
}

export function Markdown({ text }: { text: string }) {
  return (
    <div className="md selectable">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          pre: ({ children }) => <CodeBlock>{children}</CodeBlock>,
          a: ({ href, children }) => (
            <a
              href={href}
              onClick={(e) => {
                e.preventDefault();
                if (href) api.openUrl(href).catch((err: Error) => notify.err(err.message));
              }}
            >
              {children}
            </a>
          ),
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}
