import { ArrowCounterClockwise, Check, Gear, PaperPlaneRight, Sparkle, Stop, Warning, Wrench, X } from "@phosphor-icons/react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { ChatMessage } from "@/api/types";
import { useAiSettings } from "@/app/queries";
import { useNamespace, useStore } from "@/app/store";
import { Alert, Spinner } from "@/components/Basics";
import { Markdown } from "@/components/Markdown";
import { clip } from "@/lib/format";
import { useAssistant, type LiveCall, type LiveTurn } from "./store";
import "./Assistant.css";

interface Item {
  role: "user" | "assistant";
  text: string;
  calls: LiveCall[];
}

/** Regroupe les appels d'outils avec leurs résultats pour l'affichage. */
function transcript(messages: ChatMessage[]): Item[] {
  const items: Item[] = [];
  for (let i = 0; i < messages.length; i++) {
    const m = messages[i]!;
    if (m.role === "user") {
      const text = m.parts.filter((p) => p.type === "text").map((p) => (p.type === "text" ? p.text : "")).join("\n");
      if (text.trim()) items.push({ role: "user", text, calls: [] });
      continue;
    }
    const text = m.parts.filter((p) => p.type === "text").map((p) => (p.type === "text" ? p.text : "")).join("\n");
    const calls: LiveCall[] = m.parts.filter((p) => p.type === "toolCall").map((p) => (p.type === "toolCall" ? { id: p.id, name: p.name, partialJson: "", input: p.input } : { id: "", name: "", partialJson: "" }));
    const next = messages[i + 1];
    if (next && next.role === "user") {
      for (const p of next.parts) {
        if (p.type === "toolResult") {
          const c = calls.find((x) => x.id === p.callId);
          if (c) {
            c.result = p.content;
            c.isError = p.isError;
          }
        }
      }
    }
    if (text.trim() || calls.length) items.push({ role: "assistant", text, calls });
  }
  return items;
}

function argsPreview(c: LiveCall): string {
  if (c.input != null) {
    try {
      const s = JSON.stringify(c.input);
      return s === "{}" ? "" : s;
    } catch {
      return "";
    }
  }
  return c.partialJson;
}

function ToolCard({ call }: { call: LiveCall }) {
  const pending = call.result == null;
  return (
    <details className="tool-card">
      <summary>
        <Wrench size={12} />
        <span className="name">{call.name}</span>
        <span className="args">{argsPreview(call)}</span>
        {pending ? <span className="spinner" style={{ width: 11, height: 11 }} /> : call.isError ? <Warning size={13} color="var(--err)" /> : <Check size={13} color="var(--ok)" />}
      </summary>
      {call.input != null && <pre>{JSON.stringify(call.input, null, 2)}</pre>}
      {call.result != null && <pre style={{ color: call.isError ? "var(--err)" : undefined }}>{clip(call.result, 6000)}</pre>}
    </details>
  );
}

function AssistantTurn({ text, calls, thinking, streaming }: { text: string; calls: LiveCall[]; thinking?: string; streaming?: boolean }) {
  return (
    <div className="msg-assistant">
      {thinking && (
        <details className="thinking">
          <summary>Raisonnement</summary>
          {thinking}
        </details>
      )}
      {calls.map((c) => (
        <ToolCard key={c.id} call={c} />
      ))}
      {text ? <Markdown text={text} /> : null}
      {streaming && !text && calls.every((c) => c.result != null) && <Spinner label="réflexion…" />}
      {streaming && text && <span className="cursor-blink" />}
    </div>
  );
}

const QUICK = [
  "Résume l'état du cluster et signale ce qui mérite attention.",
  "Quels pods sont en erreur, et pourquoi ?",
  "Que disent les évènements récents de type Warning ?",
  "Rédige un Deployment nginx (2 répliques) avec son Service ClusterIP.",
];

export function AssistantPanel() {
  const settings = useAiSettings();
  const active = settings.data?.profiles.find((p) => p.id === settings.data?.activeProfile) ?? null;
  const { messages, live, busy, error, send, cancel, reset, usage, lastModel, lastStop, refusalCategory } = useAssistant();
  const cluster = useStore((s) => s.cluster);
  const namespace = useNamespace();
  const selection = useStore((s) => s.selection);
  const setView = useStore((s) => s.setView);
  const setAssistantOpen = useStore((s) => s.setAssistantOpen);
  const [input, setInput] = useState("");
  const bodyRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const items = useMemo(() => transcript(messages), [messages]);

  useEffect(() => {
    const el = bodyRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [items, live, error]);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const submit = () => {
    const t = input.trim();
    if (!t || busy) return;
    setInput("");
    void send(t);
  };

  return (
    <div className="assistant">
      <div className="assistant-header">
        <Sparkle size={20} color="var(--accent)" weight="fill" />
        <div className="title grow">
          <strong>Assistant</strong>
          <span className="xs muted truncate">{active ? `${active.name} · ${active.model || "modèle non choisi"}` : "aucun fournisseur configuré"}</span>
        </div>
        <button className="btn btn-ghost btn-sm btn-icon" title="Nouvelle conversation" onClick={reset} disabled={busy}>
          <ArrowCounterClockwise size={15} />
        </button>
        <button className="btn btn-ghost btn-sm btn-icon" title="Réglages de l'assistant" onClick={() => setView("settings")}>
          <Gear size={15} />
        </button>
        <button className="btn btn-ghost btn-sm btn-icon" title="Fermer (Ctrl+J)" onClick={() => setAssistantOpen(false)}>
          <X size={15} />
        </button>
      </div>

      <div className="assistant-body" ref={bodyRef}>
        {settings.data && !active && (
          <Alert tone="info">
            <div className="col gap-4">
              <div>Aucun fournisseur d'IA n'est configuré. Ajoutez Claude, ChatGPT ou un modèle local (LM Studio, Ollama…).</div>
              <button className="btn btn-sm" style={{ alignSelf: "flex-start" }} onClick={() => setView("settings")}>
                Ouvrir les réglages
              </button>
            </div>
          </Alert>
        )}
        {items.length === 0 && live.length === 0 && (
          <div className="assistant-welcome">
            <div>
              Posez une question sur {cluster ? <strong>{cluster}</strong> : "votre cluster"} : l'assistant lit les objets, les évènements, les journaux et les métriques{" "}
              {settings.data?.toolsEnabled ? "en direct" : "que vous lui montrez"}, et ne modifie jamais rien lui-même.
            </div>
            <div className="prompts">
              {selection && (
                <button className="btn btn-sm" onClick={() => send(`Explique l'état de ${selection.kind} ${selection.namespace ? selection.namespace + "/" : ""}${selection.name} et ce qui cloche, s'il y a lieu.`)} disabled={!active}>
                  Explique l'objet sélectionné ({selection.kind} {selection.name})
                </button>
              )}
              {QUICK.map((q) => (
                <button key={q} className="btn btn-sm" onClick={() => send(q)} disabled={!active}>
                  {q}
                </button>
              ))}
            </div>
          </div>
        )}
        {items.map((it, i) =>
          it.role === "user" ? (
            <div key={i} className="msg-user">
              {it.text}
            </div>
          ) : (
            <AssistantTurn key={i} text={it.text} calls={it.calls} />
          ),
        )}
        {live.map((t: LiveTurn, i) => (
          <AssistantTurn key={"live" + i} text={t.text} calls={t.calls} thinking={t.thinking} streaming={busy && i === live.length - 1} />
        ))}
        {busy && live.length === 0 && <Spinner label="envoi…" />}
        {error && <Alert tone="err">{error}</Alert>}
        {refusalCategory && <Alert tone="warn">Le fournisseur a refusé de répondre (catégorie : {refusalCategory}).</Alert>}
        {lastStop === "maxTokens" && <Alert tone="warn">Réponse tronquée : le plafond de jetons de sortie est atteint. Augmentez-le dans les réglages du profil.</Alert>}
      </div>

      <div className="assistant-context">
        <span className="tag" title="Cluster">{cluster ?? "aucun cluster"}</span>
        <span className="tag" title="Namespace">{namespace ?? "tous les namespaces"}</span>
        {selection && (
          <span className="tag truncate" title="Objet sélectionné" style={{ maxWidth: 200 }}>
            {selection.kind} {selection.name}
          </span>
        )}
      </div>
      <div className="assistant-input">
        <textarea
          ref={inputRef}
          className="input"
          rows={1}
          placeholder={active ? "Votre question…" : "Configurez un fournisseur dans les réglages"}
          value={input}
          disabled={!active}
          onChange={(e) => {
            setInput(e.target.value);
            e.target.style.height = "auto";
            e.target.style.height = Math.min(160, e.target.scrollHeight) + "px";
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
        />
        {busy ? (
          <button className="btn btn-danger btn-icon" title="Interrompre" onClick={() => cancel()}>
            <Stop size={16} weight="fill" />
          </button>
        ) : (
          <button className="btn btn-primary btn-icon" title="Envoyer (Entrée)" onClick={submit} disabled={!active || !input.trim()}>
            <PaperPlaneRight size={16} weight="fill" />
          </button>
        )}
      </div>
      <div className="assistant-footer">
        {usage.inputTokens + usage.outputTokens > 0 && (
          <span>
            {usage.inputTokens.toLocaleString("fr-FR")} jetons lus · {usage.outputTokens.toLocaleString("fr-FR")} produits
          </span>
        )}
        {lastModel && <span>· {lastModel}</span>}
      </div>
    </div>
  );
}
