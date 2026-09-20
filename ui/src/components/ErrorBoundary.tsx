// Frontière d'erreur : un écran qui plante n'emporte pas toute l'application,
// et le message exact reste lisible (et copiable) plutôt qu'un écran vide.
import { ArrowsClockwise, Copy, WarningOctagon } from "@phosphor-icons/react";
import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  /** Nom de la zone protégée, pour le message. */
  label: string;
  /** Change de valeur pour réinitialiser la frontière (ex. : l'écran courant). */
  resetKey?: unknown;
  children: ReactNode;
}

interface State {
  error: Error | null;
  stack: string;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, stack: "" };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    this.setState({ stack: `${error.stack ?? ""}\n\nComposants :${info.componentStack ?? ""}` });
    console.error(`[${this.props.label}]`, error, info.componentStack);
  }

  componentDidUpdate(prev: Props) {
    if (prev.resetKey !== this.props.resetKey && this.state.error) this.setState({ error: null, stack: "" });
  }

  render() {
    const { error, stack } = this.state;
    if (!error) return this.props.children;
    const text = `${error.name}: ${error.message}\n${stack}`;
    return (
      <div className="page">
        <div className="alert alert-err" style={{ alignItems: "flex-start" }}>
          <WarningOctagon size={20} />
          <div className="col gap-8 grow" style={{ minWidth: 0 }}>
            <strong>Une erreur a interrompu l'affichage de « {this.props.label} ».</strong>
            <div className="mono small" style={{ whiteSpace: "pre-wrap", wordBreak: "break-word" }}>
              {error.name}: {error.message}
            </div>
            <details>
              <summary className="small" style={{ cursor: "pointer" }}>Détail technique</summary>
              <pre className="code" style={{ maxHeight: 320, marginTop: 6 }}>{stack}</pre>
            </details>
            <div className="row">
              <button className="btn btn-sm" onClick={() => this.setState({ error: null, stack: "" })}>
                <ArrowsClockwise size={14} /> Réessayer
              </button>
              <button className="btn btn-sm" onClick={() => navigator.clipboard.writeText(text).catch(() => {})}>
                <Copy size={14} /> Copier le rapport
              </button>
            </div>
          </div>
        </div>
      </div>
    );
  }
}
