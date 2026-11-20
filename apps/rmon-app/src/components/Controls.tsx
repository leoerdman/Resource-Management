import { useEffect, useId, useState, type ReactNode } from "react";

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
 */
export function NumField(props: {
  label: ReactNode;
  value: number;
  onChange: (v: number) => void;
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
            props.onChange(parseNum(e.target.value));
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
