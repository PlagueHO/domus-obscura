import type { OutputPreview as Preview } from "./generated/OutputPreview";

/**
 * Show the Rust renderer's result without rendering templates in TypeScript.
 * The final application theme remains a separate user decision.
 */
export function OutputPreview({ preview }: { preview: Preview }) {
  return (
    <section aria-label="Output preview">
      <dl>
        <dt>Relative folder</dt>
        <dd>{preview.relative_folder}</dd>
        <dt>Filename</dt>
        <dd>{preview.filename}</dd>
      </dl>
      {preview.changes.length > 0 ? (
        <>
          <h3>Filename changes</h3>
          <ul>
            {preview.changes.map((change, index) => (
              <li key={`${index}:${change}`}>{change}</li>
            ))}
          </ul>
        </>
      ) : (
        <p>No filename replacements.</p>
      )}
    </section>
  );
}
