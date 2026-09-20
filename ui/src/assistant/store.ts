// ---------------------------------------------------------------------------
// Conversation avec l'assistant : historique, tours en cours de réception,
// contexte d'interface transmis à chaque question.
// ---------------------------------------------------------------------------
import { create } from "zustand";
import { api } from "@/api/client";
import type { AiContext, ChatMessage, StopReason, StreamEvent, Usage } from "@/api/types";
import { useStore, VIEW_LABELS } from "@/app/store";
import { clip } from "@/lib/format";

export interface LiveCall {
  id: string;
  name: string;
  partialJson: string;
  input?: unknown;
  result?: string;
  isError?: boolean;
}

export interface LiveTurn {
  text: string;
  thinking: string;
  calls: LiveCall[];
  done: boolean;
}

interface AssistantState {
  messages: ChatMessage[];
  live: LiveTurn[];
  busy: boolean;
  chatId: number | null;
  error: string | null;
  usage: Usage;
  lastModel: string | null;
  lastStop: StopReason | null;
  refusalCategory: string | null;
  send: (text: string) => Promise<void>;
  cancel: () => Promise<void>;
  reset: () => void;
}

let seq = 1;
const ZERO: Usage = { inputTokens: 0, outputTokens: 0 };

export function buildContext(): AiContext {
  const st = useStore.getState();
  const sel = st.selection;
  const namespace = st.cluster ? (st.namespaceByCluster[st.cluster] ?? null) : null;
  let selection: string | null = null;
  if (sel) {
    selection = `${sel.kind} ${sel.namespace ? sel.namespace + "/" : ""}${sel.name} — statut : ${sel.status}${sel.ready ? `, prêt : ${sel.ready}` : ""}`;
    if (st.selectionYaml) selection += `\n\n${clip(st.selectionYaml, 12_000)}`;
  }
  return { cluster: st.cluster, namespace, view: VIEW_LABELS[st.view], selection };
}

function freshTurn(): LiveTurn {
  return { text: "", thinking: "", calls: [], done: false };
}

/** Applique un évènement de flux aux tours en cours (sans mutation). */
export function applyEvent(live: LiveTurn[], ev: StreamEvent): { live: LiveTurn[]; error?: string; turn?: Extract<StreamEvent, { type: "turnEnd" }> } {
  const out = live.slice();
  let last = out[out.length - 1];
  if (!last || last.done) {
    last = freshTurn();
    out.push(last);
  } else {
    last = { ...last, calls: last.calls.slice() };
    out[out.length - 1] = last;
  }
  switch (ev.type) {
    case "textDelta":
      last.text += ev.text;
      break;
    case "thinkingDelta":
      last.thinking += ev.text;
      break;
    case "toolCallStart":
      last.calls.push({ id: ev.id, name: ev.name, partialJson: "" });
      break;
    case "toolCallDelta": {
      const i = last.calls.findIndex((c) => c.id === ev.id);
      if (i >= 0) last.calls[i] = { ...last.calls[i]!, partialJson: last.calls[i]!.partialJson + ev.partialJson };
      break;
    }
    case "toolCall": {
      const i = last.calls.findIndex((c) => c.id === ev.id);
      if (i >= 0) last.calls[i] = { ...last.calls[i]!, input: ev.input };
      else last.calls.push({ id: ev.id, name: ev.name, partialJson: "", input: ev.input });
      break;
    }
    case "toolResult": {
      // Le résultat concerne le dernier tour clos qui porte cet appel.
      for (let t = out.length - 1; t >= 0; t--) {
        const turn = out[t]!;
        const i = turn.calls.findIndex((c) => c.id === ev.callId);
        if (i >= 0) {
          const calls = turn.calls.slice();
          calls[i] = { ...calls[i]!, result: ev.content, isError: ev.isError };
          out[t] = { ...turn, calls };
          break;
        }
      }
      // Un résultat n'ouvre pas de nouveau tour vide.
      if (out[out.length - 1] === last && !last.text && last.calls.length === 0 && !last.thinking) out.pop();
      break;
    }
    case "turnEnd":
      last.done = true;
      return { live: out, turn: ev };
    case "error":
      if (!last.text && last.calls.length === 0 && !last.thinking) out.pop();
      return { live: out, error: ev.message };
  }
  return { live: out };
}

/** Convertit des tours partiels en messages, pour garder une réponse interrompue. */
function liveToMessages(live: LiveTurn[]): ChatMessage[] {
  const out: ChatMessage[] = [];
  for (const t of live) {
    const parts: ChatMessage["parts"] = [];
    if (t.text) parts.push({ type: "text", text: t.text });
    for (const c of t.calls) parts.push({ type: "toolCall", id: c.id, name: c.name, input: c.input ?? {} });
    if (parts.length) out.push({ role: "assistant", parts });
    const results = t.calls.filter((c) => c.result != null);
    if (results.length) {
      out.push({ role: "user", parts: results.map((c) => ({ type: "toolResult", callId: c.id, name: c.name, content: c.result ?? "", isError: !!c.isError })) });
    }
  }
  return out;
}

export const useAssistant = create<AssistantState>((set, get) => ({
  messages: [],
  live: [],
  busy: false,
  chatId: null,
  error: null,
  usage: ZERO,
  lastModel: null,
  lastStop: null,
  refusalCategory: null,

  reset: () => set({ messages: [], live: [], error: null, usage: ZERO, lastModel: null, lastStop: null, refusalCategory: null }),

  cancel: async () => {
    const id = get().chatId;
    if (id != null) await api.ai.cancel(id).catch(() => {});
  },

  send: async (text) => {
    const t = text.trim();
    if (!t || get().busy) return;
    const user: ChatMessage = { role: "user", parts: [{ type: "text", text: t }] };
    const history = [...get().messages, user];
    const chatId = seq++;
    set({ messages: history, live: [], busy: true, chatId, error: null, refusalCategory: null, lastStop: null });

    const onEvent = (ev: StreamEvent) => {
      set((s) => {
        const r = applyEvent(s.live, ev);
        const patch: Partial<AssistantState> = { live: r.live };
        if (r.error) patch.error = r.error;
        if (r.turn) {
          patch.lastModel = r.turn.model ?? s.lastModel;
          patch.lastStop = r.turn.stopReason;
          patch.refusalCategory = r.turn.refusalCategory;
        }
        return patch;
      });
    };

    try {
      const out = await api.ai.chat(chatId, history, buildContext(), null, onEvent);
      set((s) => ({
        messages: [...s.messages, ...out.messages],
        live: [],
        busy: false,
        chatId: null,
        usage: { inputTokens: s.usage.inputTokens + out.usage.inputTokens, outputTokens: s.usage.outputTokens + out.usage.outputTokens },
        lastModel: out.model ?? s.lastModel,
        lastStop: out.stopReason,
        refusalCategory: out.refusalCategory,
      }));
    } catch (e) {
      const message = (e as Error).message;
      set((s) => ({
        messages: [...s.messages, ...liveToMessages(s.live)],
        live: [],
        busy: false,
        chatId: null,
        error: message,
      }));
    }
  },
}));
