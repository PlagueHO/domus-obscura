import { useState } from "react";
import type { Answer } from "../src/generated/Answer";
import type { QuestionDetails } from "../src/generated/QuestionDetails";
import type { StoredQuestion } from "../src/generated/StoredQuestion";
import { request } from "./api";
import { useCommand, useResource } from "./hooks";
import { Alert, Badge, Button, Card, Empty, Pager } from "./components/ui";
import { stateLabel } from "./state";

export function Inbox({
  questions,
  loading,
  error,
  selectedId,
  refresh,
  previous,
  next,
}: {
  questions: StoredQuestion[];
  loading: boolean;
  error: unknown;
  selectedId?: number | undefined;
  refresh: () => void;
  previous?: (() => void) | undefined;
  next?: (() => void) | undefined;
}) {
  return (
    <>
      <header className="hero request-hero">
        <div>
          <h1>Decisions</h1>
          <p>
            Saved questions and answers that still need a separate recovery action. Nothing is
            answered, resumed or downloaded automatically.
          </p>
        </div>
        <Button variant="secondary" disabled={loading} onClick={refresh}>
          Refresh decisions
        </Button>
      </header>
      <Alert error={error} />
      {selectedId !== undefined && (
        <QuestionDetail key={selectedId} id={selectedId} onChanged={refresh} />
      )}
      <Card title="Requests waiting for a decision">
        {loading && <p role="status">Reading saved decisions…</p>}
        {questions.map((question) => (
          <div className="list-row" key={question.id}>
            <div>
              <a href={`#/inbox/${question.id}`}>
                Request #{question.job_id} · Plan {question.plan_version}
              </a>
              <p>{question.prompt}</p>
              {question.answer !== null && <p>Saved answer: {question.answer}</p>}
            </div>
            <Badge>{question.answer === null ? "Answer needed" : "Recovery action needed"}</Badge>
          </div>
        ))}
        {!loading && !error && questions.length === 0 && (
          <Empty>No requests are waiting in this page of decisions.</Empty>
        )}
        <Pager busy={loading} previous={previous} next={next} />
      </Card>
    </>
  );
}

function QuestionDetail({ id, onChanged }: { id: number; onChanged: () => void }) {
  const details = useResource(
    (signal) => request<QuestionDetails>(`/api/questions/${id}`, { signal }),
    String(id),
    15000,
  );
  function refresh() {
    details.refresh();
    onChanged();
  }
  return (
    <Card title={`Decision #${id}`}>
      <Button variant="secondary" disabled={details.loading} onClick={refresh}>
        Reload this decision
      </Button>
      <Alert error={details.error} />
      {details.loading && !details.data && <p role="status">Reading this saved decision…</p>}
      {details.data && (
        <QuestionActions
          key={`${id}:${details.data.question.answer ?? ""}:${details.data.current}`}
          details={details.data}
          disabled={details.loading || Boolean(details.error)}
          onChanged={refresh}
        />
      )}
    </Card>
  );
}

export function recoveryCommand(
  details: QuestionDetails,
): { path: string; plan_version: number } | null {
  if (!details.current || details.question.answer === null) return null;
  const { recovery, question } = details;
  switch (recovery.kind) {
    case "artwork":
      return recovery.decision.resumed_at === null
        ? {
            path: `/api/questions/${question.id}/artwork/resume`,
            plan_version: question.plan_version,
          }
        : null;
    case "job_control":
      return recovery.control.continued_at === null
        ? {
            path: `/api/job-controls/${recovery.control.id}/continue`,
            plan_version: question.plan_version,
          }
        : null;
    case "album_restore":
    case "manual":
      return null;
  }
}

export function QuestionActions({
  details,
  disabled,
  onChanged,
}: {
  details: QuestionDetails;
  disabled: boolean;
  onChanged: () => void;
}) {
  const { question, recovery } = details;
  const [choice, setChoice] = useState("");
  const [submitted, setSubmitted] = useState(false);
  const [notice, setNotice] = useState("");
  const command = useCommand();
  const blocked = disabled || command.busy || submitted || !details.current;
  const continuation = recoveryCommand(details);

  async function answer() {
    if (blocked || question.answer !== null || !question.choices.includes(choice)) return;
    const body: Answer = { plan_version: question.plan_version, choice };
    await request(`/api/questions/${question.id}/answer`, { body });
    setSubmitted(true);
    setNotice(
      recovery.kind === "album_restore"
        ? "Answer saved. No files changed. Open the request to review the saved restore state."
        : "Answer saved. Work is still paused. Reload this decision before the separate recovery action.",
    );
    onChanged();
  }

  async function resume() {
    if (blocked || !continuation) return;
    await request(continuation.path, { body: { plan_version: continuation.plan_version } });
    setSubmitted(true);
    setNotice("Saved answer applied. This command did not start a worker or a download.");
    onChanged();
  }

  return (
    <>
      <p>
        <a href={`#/request/${question.job_id}/${question.plan_version}`}>
          Open request #{question.job_id} · Plan {question.plan_version}
        </a>
        {" · "}
        {stateLabel(details.job_state)}
      </p>
      <p>{question.prompt}</p>
      {!details.current && (
        <p role="status">
          This decision is no longer active. The current plan is {details.current_plan_version}. No
          answer or recovery action is available here.
        </p>
      )}
      {recovery.kind === "artwork" && (
        <p>
          Open the request to prepare and approve the exact JPEG cover. Permission, preview
          approval, this answer and recovery are separate actions. Saved transfers stay intact.
        </p>
      )}
      {recovery.kind === "album_restore" && (
        <p>
          Restore destination {recovery.location_id} needs review. Saved state: {recovery.state}.
          Open the request to read a verified preview or continue the exact approved restore. Saving
          this answer does not approve files or continue restore. The acquisition remains complete.
        </p>
      )}
      {question.answer === null ? (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void command.run(answer);
          }}
        >
          <label>
            Your answer
            <select
              value={choice}
              disabled={blocked}
              onChange={(event) => setChoice(event.target.value)}
            >
              <option value="" disabled>
                Choose one offered answer
              </option>
              {question.choices.map((offered) => (
                <option key={offered} value={offered}>
                  {offered}
                </option>
              ))}
            </select>
          </label>
          <p>Saving an answer does not resume work or start a download.</p>
          <Button type="submit" disabled={blocked || !question.choices.includes(choice)}>
            Save answer
          </Button>
        </form>
      ) : (
        <>
          <p>
            <strong>Saved answer:</strong> {question.answer}
          </p>
          {continuation && (
            <>
              <p>
                The server rechecks the current plan and safe state before applying this saved
                answer. This does not start media work. Use the request's separate start control
                only when it is available.
              </p>
              <Button disabled={blocked} onClick={() => void command.run(resume)}>
                {recovery.kind === "artwork"
                  ? "Apply approved cover and restore safe state"
                  : "Apply saved pause answer"}
              </Button>
            </>
          )}
          {details.current && recovery.kind === "manual" && (
            <p className="warning">
              The answer is saved. This decision has no supported browser recovery action. Use the
              matching operator recovery command. Do not retry an uncertain transfer.
            </p>
          )}
        </>
      )}
      {notice && <p role="status">{notice}</p>}
      <Alert error={command.error} />
      {Boolean(command.error) && (
        <p>
          Your selection is kept. Refresh the saved decision before trying again. Commands never
          retry automatically.
        </p>
      )}
    </>
  );
}
