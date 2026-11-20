import { useEffect, useId, useRef, useState, type ReactNode } from "react";

/** macOS-style switch (a styled checkbox: keyboard and screen readers work as usual). */
export function Switch(props: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: ReactNode;
  hint?: ReactNode;
  disabled?: boolean;
  big?: boolean;
}) {
  const id = useId();
  return (
    <div className={props.big ? "switch-row big" : "switch-row"}>
      <label htmlFor={id} className="switch-label">
        <span>{props.label}</span>
        {props.hint && <span className="dim small">{props.hint}</span>}
      </label>
      <input
        id={id}
        type="checkbox"
        role="switch"
        className="switch"
        checked={props.checked}
        disabled={props.disabled}
        onChange={(e) => props.onChange(e.target.checked)}
      />
    </div>
  );
}

const parseNum = (s: string) => {
  const t = s.trim().replace(",", ".");
  return t === "" ? NaN : Number(t);
};
const showNum = (v: number) => (Number.isNaN(v) ? "" : String(v).replace(".", ","));

/**
 * Number field that accepts a Russian decimal comma. Keeps the typed text while it is
 * half-written ("14,"); an empty or broken value is reported as NaN.
 *
 * `onChange` reports every keystroke (forms with a Save button). `onCommit` applies the value
 * on Enter or when the field loses focus (live settings); a broken value or a refused commit
 * (`false`) puts the last good value back.
 */
export function NumField(props: {
  label: ReactNode;
  value: number;
  onChange?: (v: number) => void;
  onCommit?: (v: number) => boolean | Promise<boolean>;
  unit?: string;
  hint?: ReactNode;
  min?: number;
}) {
  const id = useId();
  const [text, setText] = useState(() => showNum(props.value));
  useEffect(() => {
    const n = parseNum(text);
    if (!(n === props.value || (Number.isNaN(n) && Number.isNaN(props.value)))) setText(showNum(props.value));
    // Only an outside change of `value` (reset, load) should rewrite the text.
  }, [props.value]);
  const n = parseNum(text);
  const bad = Number.isNaN(n) || (props.min != null && n < props.min);

  // Enter and the blur after it must not save the same value twice.
  const inFlight = useRef<number | null>(null);
  async function commit() {
    if (!props.onCommit) return;
    const v = parseNum(text);
    if (v === props.value || v === inFlight.current) return;
    inFlight.current = v;
    try {
      const ok = Number.isFinite(v) && v >= (props.min ?? 0) && (await props.onCommit(v));
      if (!ok) setText(showNum(props.value));
    } finally {
      inFlight.current = null;
    }
  }

  return (
    <div className="field">
      <label htmlFor={id}>{props.label}</label>
      <span className="field-input">
        <input
          id={id}
          type="text"
          inputMode="decimal"
          className={bad ? "num-input bad" : "num-input"}
          value={text}
          onChange={(e) => {
            setText(e.target.value);
            props.onChange?.(parseNum(e.target.value));
          }}
          onBlur={() => void commit()}
          onKeyDown={(e) => {
            if (e.key === "Enter" && props.onCommit) {
              e.preventDefault();
              void commit();
            } else if (e.key === "Escape" && props.onCommit) {
              setText(showNum(props.value));
            }
          }}
          spellCheck={false}
        />
        {props.unit && <span className="dim">{props.unit}</span>}
      </span>
      {props.hint && <span className="dim small field-hint">{props.hint}</span>}
    </div>
  );
}

export function SelectField<T extends string | number>(props: {
  label: ReactNode;
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
}) {
  const id = useId();
  return (
    <div className="field">
      <label htmlFor={id}>{props.label}</label>
      <span className="field-input">
        <select
          id={id}
          className="select"
          value={String(props.value)}
          onChange={(e) => {
            const opt = props.options.find((o) => String(o.value) === e.target.value);
            if (opt) props.onChange(opt.value);
          }}
        >
          {props.options.map((o) => (
            <option key={String(o.value)} value={String(o.value)}>
              {o.label}
            </option>
          ))}
        </select>
      </span>
    </div>
  );
}
