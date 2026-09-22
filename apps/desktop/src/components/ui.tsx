import { X } from "lucide-react";
import { useEffect, useId, useRef, type ButtonHTMLAttributes, type InputHTMLAttributes, type ReactNode, type SelectHTMLAttributes } from "react";
import { useTranslation } from "react-i18next";

const cx = (...c: (string | false | undefined)[]) => c.filter(Boolean).join(" ");

type Variant = "primary" | "secondary" | "ghost" | "danger";

const variants: Record<Variant, string> = {
  primary: "bg-accent text-accent-fg hover:bg-accent-strong active:scale-[0.98] shadow-sm",
  secondary: "bg-surface-2 text-fg hover:bg-line border border-line active:scale-[0.98]",
  ghost: "text-fg hover:bg-surface-2 active:scale-[0.98]",
  danger: "bg-danger text-white hover:opacity-90 active:scale-[0.98]",
};

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: "sm" | "md" | "lg";
  icon?: ReactNode;
}

export function Button({ variant = "secondary", size = "md", icon, className, children, ...rest }: ButtonProps) {
  const sizes = { sm: "h-8 px-3 text-[13px]", md: "h-9 px-4", lg: "h-11 px-6 text-[15px]" }[size];
  return (
    <button
      {...rest}
      className={cx(
        "inline-flex items-center justify-center gap-2 rounded-lg font-medium transition duration-150 disabled:pointer-events-none disabled:opacity-50",
        variants[variant],
        sizes,
        className,
      )}
    >
      {icon}
      {children}
    </button>
  );
}

/** Icon-only button: always carries an accessible name and a visible tooltip. */
export function IconButton({ label, active, children, className, ...rest }: ButtonHTMLAttributes<HTMLButtonElement> & { label: string; active?: boolean }) {
  return (
    <button
      {...rest}
      aria-label={label}
      title={label}
      aria-pressed={active}
      className={cx(
        "grid size-9 place-items-center rounded-lg transition duration-150 hover:bg-surface-2 active:scale-95 disabled:opacity-40",
        active && "bg-accent-soft text-accent",
        className,
      )}
    >
      {children}
    </button>
  );
}

export function Toggle({ checked, onChange, label, hint, disabled }: { checked: boolean; onChange: (v: boolean) => void; label: string; hint?: string; disabled?: boolean }) {
  const id = useId();
  return (
    <div className="flex items-start justify-between gap-4 py-2.5">
      <label htmlFor={id} className="min-w-0 flex-1">
        <span className="block font-medium">{label}</span>
        {hint && <span className="block text-[13px] text-muted">{hint}</span>}
      </label>
      <button
        id={id}
        role="switch"
        aria-checked={checked}
        disabled={disabled}
        onClick={() => onChange(!checked)}
        className={cx(
          "relative mt-0.5 h-6 w-11 shrink-0 rounded-full border transition-colors duration-150 disabled:opacity-50",
          checked ? "border-accent bg-accent" : "border-line bg-surface-2",
        )}
      >
        <span
          className={cx(
            "absolute top-0.5 size-4.5 rounded-full bg-white shadow transition-all duration-150",
            checked ? "start-[calc(100%-1.375rem)]" : "start-0.5",
          )}
        />
      </button>
    </div>
  );
}

export function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-2 py-2.5">
      <div className="min-w-[14rem] flex-1">
        <div className="font-medium">{label}</div>
        {hint && <div className="text-[13px] text-muted">{hint}</div>}
      </div>
      <div className="w-full max-w-xs sm:w-auto">{children}</div>
    </div>
  );
}

const inputClass =
  "h-9 w-full rounded-lg border border-line bg-surface px-3 text-fg placeholder:text-muted/70 transition focus:border-accent focus:outline-none focus:ring-2 focus:ring-accent/30";

export function TextInput({ className, ...rest }: InputHTMLAttributes<HTMLInputElement>) {
  return <input {...rest} className={cx(inputClass, className)} />;
}

export function Select({ className, children, ...rest }: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select {...rest} className={cx(inputClass, "min-w-40 cursor-pointer pe-8", className)}>
      {children}
    </select>
  );
}

export function Modal({ title, children, onClose, footer, dismissible = true, wide }: { title: string; children: ReactNode; onClose?: () => void; footer?: ReactNode; dismissible?: boolean; wide?: boolean }) {
  const { t } = useTranslation();
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ref.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && dismissible) onClose?.();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [dismissible, onClose]);
  return (
    <div className="anim-fade fixed inset-0 z-50 grid place-items-center p-4 backdrop-blur-[2px]" style={{ background: "var(--overlay)" }} role="presentation">
      <div
        ref={ref}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className={cx("anim-pop max-h-[90vh] w-full overflow-auto rounded-2xl border border-line bg-surface p-6 shadow-card outline-none", wide ? "max-w-2xl" : "max-w-md")}
      >
        <div className="mb-4 flex items-start justify-between gap-4">
          <h2 className="text-lg font-semibold">{title}</h2>
          {dismissible && onClose && (
            <IconButton label={t("common.close")} onClick={onClose} className="-m-1.5">
              <X size={18} />
            </IconButton>
          )}
        </div>
        {children}
        {footer && <div className="mt-6 flex flex-wrap justify-end gap-2">{footer}</div>}
      </div>
    </div>
  );
}

/** Status indicator that never relies on colour alone: icon shape plus text label. */
export function StatusDot({ tone, pulse }: { tone: "ok" | "warn" | "danger" | "muted"; pulse?: boolean }) {
  const colour = { ok: "bg-ok", warn: "bg-warn", danger: "bg-danger", muted: "bg-muted" }[tone];
  return <span aria-hidden className={cx("inline-block size-2.5 rounded-full", colour)} style={pulse ? { animation: "pulse-ring 1.6s infinite" } : undefined} />;
}

export function Section({ title, children, aside }: { title: string; children: ReactNode; aside?: ReactNode }) {
  return (
    <section className="mb-6">
      <div className="mb-2 flex items-center justify-between">
        <h3 className="text-[13px] font-semibold uppercase tracking-wide text-muted">{title}</h3>
        {aside}
      </div>
      <div className="divide-y divide-line rounded-xl border border-line bg-surface px-4">{children}</div>
    </section>
  );
}

export function Toast({ message, tone = "info", onDone }: { message: string; tone?: "info" | "error"; onDone: () => void }) {
  useEffect(() => {
    const id = window.setTimeout(onDone, 4500);
    return () => window.clearTimeout(id);
  }, [onDone]);
  return (
    <div
      role="status"
      className={cx(
        "anim-pop pointer-events-auto rounded-xl border px-4 py-2.5 shadow-card",
        tone === "error" ? "border-danger/40 bg-surface text-danger" : "border-line bg-surface text-fg",
      )}
    >
      {message}
    </div>
  );
}

export { cx };
