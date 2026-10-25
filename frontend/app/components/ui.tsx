import type { ButtonHTMLAttributes, ReactNode } from "react";
import { ApiError, errorText } from "../api";

// shadcn/ui-style owned primitives: native semantics, data slots and variants.
// No runtime CSS injection or third-party component installation is required.
export function Button({
  variant = "default",
  className = "",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "default" | "secondary" | "destructive" | "ghost";
}) {
  return (
    <button
      type="button"
      data-slot="button"
      className={`button button-${variant} ${className}`}
      {...props}
    />
  );
}

export function Card({
  title,
  children,
  className = "",
}: {
  title: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section data-slot="card" className={`panel ${className}`}>
      <h2>{title}</h2>
      {children}
    </section>
  );
}

export function Alert({ error }: { error: unknown }) {
  if (!error) return null;
  return (
    <div role="alert" data-slot="alert" className="alert">
      <strong>The request could not finish.</strong>
      <p>{errorText(error)}</p>
      {error instanceof ApiError && error.status === 401 ? (
        <a href="/auth/login">Sign in with Microsoft</a>
      ) : error instanceof ApiError && error.retryAfterSeconds === 4294967295 ? (
        <p>
          Provider requests are paused. Review the service before resuming. Repeating the action
          will not clear this pause.
        </p>
      ) : error instanceof ApiError && error.retryAfterSeconds !== null ? (
        <p>
          At the time of this response, the service required a wait of at least{" "}
          {error.retryAfterSeconds} {error.retryAfterSeconds === 1 ? "second" : "seconds"}. After
          that wait, try the action again. It will not retry automatically.
        </p>
      ) : error instanceof ApiError && (error.status === 429 || error.status === 503) ? (
        <p>The service is busy or unavailable. Try the action later.</p>
      ) : (
        <p>Check the service or setup, then try the action again.</p>
      )}
    </div>
  );
}

export function Empty({ children }: { children: ReactNode }) {
  return <p className="empty">{children}</p>;
}

export function Badge({ children }: { children: ReactNode }) {
  return (
    <span data-slot="badge" className="badge">
      {children}
    </span>
  );
}

export function Check({
  checked,
  onChange,
  children,
  disabled = false,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
  children: ReactNode;
  disabled?: boolean;
}) {
  return (
    <label className="check">
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span>{children}</span>
    </label>
  );
}

export function Pager({
  previous,
  next,
  busy = false,
}: {
  previous?: (() => void) | undefined;
  next?: (() => void) | undefined;
  busy?: boolean;
}) {
  return (
    <nav aria-label="Page controls" className="actions">
      <Button variant="secondary" disabled={!previous || busy} onClick={previous}>
        Previous page
      </Button>
      <Button variant="secondary" disabled={!next || busy} onClick={next}>
        Next page
      </Button>
    </nav>
  );
}

export function Cover({ title }: { title: string }) {
  return (
    <div className="cover-placeholder" aria-label={`No artwork loaded for ${title}`}>
      <svg viewBox="0 0 64 64" aria-hidden="true">
        <circle cx="32" cy="32" r="25" />
        <circle cx="32" cy="32" r="6" />
        <path d="M13 28a20 20 0 0 1 15-15M51 36a20 20 0 0 1-15 15" />
      </svg>
    </div>
  );
}
