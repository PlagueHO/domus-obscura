import {
  createContext,
  useContext,
  useId,
  useSyncExternalStore,
  type ButtonHTMLAttributes,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import {
  ApiError,
  errorText,
  getProviderSnapshot,
  getIdleProviderSnapshot,
  subscribeProvider,
} from "../api";
import type { CatalogStatus } from "../../src/generated/CatalogStatus";

export function providerWaitText(status?: CatalogStatus): string {
  if (!status)
    return "Checking the shared provider budget. Metadata requests can wait up to 15 seconds before submission.";
  if (status.state === "paused")
    return "Provider requests are paused. Review the service before resuming.";
  if (status.state === "saturated")
    return "Provider request capacity is full. Try the metadata action later.";
  if (status.state === "active" && status.known_retry_after_seconds === null)
    return "A provider request is active. Its completion time is unknown.";
  if (status.known_retry_after_seconds !== null && status.known_retry_after_seconds > 0)
    return `Waiting for the shared provider budget. At the last check, at least ${status.known_retry_after_seconds} seconds remained. This estimate can change.`;
  return "The provider budget is available. The metadata request is pending.";
}

export function ProviderWaiting() {
  const snapshot = useSyncExternalStore(
    subscribeProvider,
    getProviderSnapshot,
    getIdleProviderSnapshot,
  );
  if (!snapshot.pending) return null;
  return <ProviderWaitNotice status={snapshot.status} error={snapshot.error} />;
}

export function ProviderWaitNotice({
  status,
  error,
}: {
  status?: CatalogStatus | undefined;
  error?: unknown;
}) {
  return (
    <div className="alert" data-slot="provider-wait" role="status">
      <strong>Metadata request pending</strong>
      <p>{providerWaitText(status)}</p>
      {error !== undefined && <p>Provider status could not be read: {errorText(error)}</p>}
      <p>No download or approval starts from this wait.</p>
    </div>
  );
}

// shadcn/ui-style owned primitives: native semantics, data slots and variants.
// No runtime CSS injection or third-party component installation is required.
export function Button({
  variant = "default",
  className = "",
  width,
  icon,
  busy = false,
  disabled,
  children,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "default" | "secondary" | "destructive" | "ghost";
  width?: "small" | "medium" | "large" | undefined;
  icon?: ReactNode;
  busy?: boolean;
}) {
  return (
    <button
      type="button"
      data-slot="button"
      className={`button button-${variant} ${className}`}
      data-width={width}
      {...props}
      disabled={disabled || busy}
      aria-busy={busy ? true : props["aria-busy"]}
    >
      {(icon || busy) && (
        <span className="button-icon" aria-hidden="true">
          {busy ? <span className="button-spinner" /> : icon}
        </span>
      )}
      {children}
    </button>
  );
}

export const SettingsSaveTarget = createContext<HTMLDivElement | null>(null);

export function SettingsSaveButton(
  props: ButtonHTMLAttributes<HTMLButtonElement> & {
    busy?: boolean;
  },
) {
  const target = useContext(SettingsSaveTarget);
  const button = (
    <Button
      {...props}
      width="large"
      icon={
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.7"
          aria-hidden="true"
        >
          <path d="M4 3h13l4 4v14H3V3h1Zm3 0v6h10V3M7 21v-8h10v8" />
        </svg>
      }
    />
  );
  return target ? createPortal(button, target) : button;
}

export function HeaderIconAction({
  label,
  children,
  ...action
}: { label: string; children: ReactNode } & (
  | { href: string; onClick?: never; disabled?: never }
  | { href?: never; onClick: () => void; disabled?: boolean }
)) {
  const attributes = {
    className: "icon-button header-icon-action",
    "aria-label": label,
    title: label,
  };
  return action.href !== undefined ? (
    <a {...attributes} href={action.href}>
      {children}
    </a>
  ) : (
    <button {...attributes} type="button" onClick={action.onClick} disabled={action.disabled}>
      {children}
    </button>
  );
}

export function Subcard({
  title,
  actions,
  children,
}: {
  title: string;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="subcard" aria-label={title}>
      <header className="subcard-header">
        <h3>{title}</h3>
        {actions}
      </header>
      {children}
    </section>
  );
}

export function CompactTable({
  label,
  columns,
  children,
}: {
  label: string;
  columns: string[];
  children: ReactNode;
}) {
  return (
    <table className="compact-table" aria-label={label}>
      <thead>
        <tr>
          {columns.map((column) => (
            <th key={column} scope="col">
              <span className="visually-hidden">{column}</span>
            </th>
          ))}
        </tr>
      </thead>
      <tbody>{children}</tbody>
    </table>
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

/**
 * Compact titled card. Without children it renders the header only, so a
 * collapsed summary card needs no empty body.
 */
export function HeaderCard({
  title,
  actions,
  children,
  className = "",
}: {
  title: string;
  actions?: ReactNode;
  children?: ReactNode;
  className?: string;
}) {
  return (
    <section data-slot="header-card" className={`design-card ${className}`}>
      <header className="design-card-header">
        <h2>{title}</h2>
        {actions}
      </header>
      {children ? <div className="design-card-body">{children}</div> : null}
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
      ) : error instanceof ApiError && error.providerState === "active" ? (
        <p>
          A provider request is active. Its completion time is unknown. Try the metadata action
          later. It will not retry automatically.
        </p>
      ) : error instanceof ApiError && error.providerState === "saturated" ? (
        <p>
          Provider request capacity is full. Try the metadata action later. It will not retry
          automatically.
        </p>
      ) : error instanceof ApiError &&
        (error.providerState === "paused" || error.retryAfterSeconds === 4294967295) ? (
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
      ) : error instanceof ApiError && (error.status === 400 || error.status === 409) ? null : (
        <p>Check the service or setup, then try the action again.</p>
      )}
    </div>
  );
}

export function Empty({ children }: { children: ReactNode }) {
  return <p className="empty">{children}</p>;
}

export type Tone = "neutral" | "info" | "success" | "warning" | "danger";

export function Badge({ children, tone = "neutral" }: { children: ReactNode; tone?: Tone }) {
  return (
    <span data-slot="badge" className={`badge badge-${tone}`}>
      {children}
    </span>
  );
}

/** The one status vocabulary: every status badge maps its state to a tone here. */
const STATUS_TONES = {
  ready: "success",
  complete: "success",
  scanning: "info",
  pending: "neutral",
  attention: "warning",
  failed: "danger",
} as const satisfies Record<string, Tone>;

export type Status = keyof typeof STATUS_TONES;

export function StatusBadge({ status, children }: { status: Status; children?: ReactNode }) {
  return <Badge tone={STATUS_TONES[status]}>{children ?? status}</Badge>;
}

/** One settings line: label and hint on the left, control or value on the right. */
export function SettingsRow({
  label,
  hint,
  children,
  wide = false,
}: {
  label: string;
  hint?: ReactNode;
  children?: ReactNode | ((control: { id: string; "aria-describedby"?: string }) => ReactNode);
  wide?: boolean;
}) {
  const id = useId();
  const field = typeof children === "function";
  return (
    <div data-slot="settings-row" data-layout={wide ? "wide" : "inline"}>
      {field ? <label htmlFor={id}>{label}</label> : <strong>{label}</strong>}
      <div className="settings-row-control">
        {field ? children(hint ? { id, "aria-describedby": `${id}-hint` } : { id }) : children}
      </div>
      {hint && (
        <p className="settings-row-hint muted" id={field ? `${id}-hint` : undefined}>
          {hint}
        </p>
      )}
    </div>
  );
}

/** A compact number with a label, for counts and totals. */
export function StatTile({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div data-slot="stat-tile" className="stat-tile">
      <span className="stat-value">{value}</span>
      <span className="muted">{label}</span>
    </div>
  );
}

/** Designed empty state: says what is missing and, when useful, what to do next. */
export function EmptyState({
  title,
  children,
  action,
}: {
  title: string;
  children?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div data-slot="empty-state" className="empty-state">
      <strong>{title}</strong>
      {children ? <p className="muted">{children}</p> : null}
      {action}
    </div>
  );
}

export function LoadingState({ label = "Loading" }: { label?: string }) {
  return (
    <div data-slot="loading-state" className="loading-state" role="status">
      <span className="spinner" aria-hidden="true" />
      {label}
    </div>
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

export function Switch({
  checked,
  onChange,
  ...attributes
}: {
  id: string;
  "aria-describedby"?: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <input
      {...attributes}
      className="switch"
      type="checkbox"
      role="switch"
      checked={checked}
      onChange={(event) => onChange(event.target.checked)}
    />
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
