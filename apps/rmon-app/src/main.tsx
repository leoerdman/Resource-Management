import { Component, StrictMode, type ErrorInfo, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./styles.css";

/**
 * Последний рубеж: любая ошибка — текстом в окне, а не пустой экран. В релизной сборке нет
 * инструментов разработчика, и без этого сбой интерфейса выглядел бы как «окно не загрузилось».
 */
function crashScreen(title: string, detail: unknown) {
  const root = document.getElementById("root");
  if (!root) return;
  const text = detail instanceof Error ? `${detail.name}: ${detail.message}\n\n${detail.stack ?? ""}` : String(detail);
  root.innerHTML = "";
  const box = document.createElement("div");
  box.className = "crash";
  const h = document.createElement("h2");
  h.textContent = title;
  const pre = document.createElement("pre");
  pre.textContent = text;
  const btn = document.createElement("button");
  btn.className = "btn";
  btn.textContent = "Перезагрузить окно";
  btn.onclick = () => location.reload();
  box.append(h, pre, btn);
  root.append(box);
}

let mounted = false;
window.addEventListener("error", (e) => {
  if (!mounted) crashScreen("Окно не запустилось", e.error ?? e.message);
});
window.addEventListener("unhandledrejection", (e) => {
  if (!mounted) crashScreen("Окно не запустилось", e.reason);
});

class Boundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(error, info.componentStack);
  }
  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="crash">
        <h2>Сбой интерфейса</h2>
        <pre>{`${error.name}: ${error.message}\n\n${error.stack ?? ""}`}</pre>
        <button className="btn" onClick={() => location.reload()}>
          Перезагрузить окно
        </button>
      </div>
    );
  }
}

try {
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <Boundary>
        <App />
      </Boundary>
    </StrictMode>,
  );
  mounted = true;
} catch (e) {
  crashScreen("Окно не запустилось", e);
}
