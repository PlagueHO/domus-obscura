import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { QuestionDetails } from "../src/generated/QuestionDetails";
import { Inbox, QuestionActions, recoveryCommand } from "./Inbox";

function details(overrides: Partial<QuestionDetails> = {}): QuestionDetails {
  return {
    question: {
      id: 7,
      job_id: 3,
      plan_version: 2,
      prompt: "Choose a safe recovery action",
      choices: ["Resume", "Cancel"],
      answer: null,
    },
    job_state: "awaiting_input",
    current_plan_version: 2,
    current: true,
    recovery: {
      kind: "job_control",
      control: {
        id: 11,
        job_id: 3,
        plan_version: 2,
        action: "pause",
        requested_at: 1,
        applied_at: 2,
        question_id: 7,
        paused_state: "queued",
        continued_at: null,
      },
    },
    ...overrides,
  };
}

function render(value: QuestionDetails, disabled = false) {
  return renderToStaticMarkup(
    <QuestionActions details={value} disabled={disabled} onChanged={() => {}} />,
  );
}

describe("persistent decision inbox", () => {
  it("links destination-bound restore questions without granting approval or continuation", () => {
    const value = details({
      job_state: "complete",
      recovery: {
        kind: "album_restore",
        destination_id: 21,
        location_id: 4,
        state: "restore_prepared",
      },
    });
    const markup = render(value);
    expect(markup).toContain("Restore destination 4");
    expect(markup).toContain("restore_prepared");
    expect(markup).toContain("does not approve files or continue restore");
    value.question.answer = "Review restore state";
    expect(recoveryCommand(value)).toBeNull();
    expect(render(value)).not.toContain("Apply saved pause answer");
  });
  it("keeps an answer explicit and does not expose resume before it is saved", () => {
    const value = details();
    const markup = render(value);
    expect(markup).toContain("Choose one offered answer");
    expect(markup).toContain("Save answer");
    expect(markup).toContain("Saving an answer does not resume work");
    expect(markup).not.toContain("Apply saved pause answer");
    expect(markup).toMatch(/<button[^>]*disabled/);
    expect(recoveryCommand(value)).toBeNull();
  });

  it("uses the actual control ID and frozen version for the separate recovery action", () => {
    const value = details();
    value.question.answer = "Resume";
    expect(recoveryCommand(value)).toEqual({
      path: "/api/job-controls/11/continue",
      plan_version: 2,
    });
    const markup = render(value);
    expect(markup).toContain("Apply saved pause answer");
    expect(markup).not.toContain("Save answer");
  });

  it("does not infer resume from generic prompt or choice text", () => {
    const value = details({ recovery: { kind: "manual" } });
    value.question.answer = "Resume";
    expect(recoveryCommand(value)).toBeNull();
    expect(render(value)).toContain("no supported browser recovery action");
  });

  it("blocks obsolete decisions and cached actions during load or error", () => {
    const value = details({ current: false, current_plan_version: 3 });
    value.question.answer = "Resume";
    expect(recoveryCommand(value)).toBeNull();
    expect(render(value)).toContain("This decision is no longer active");
    const active = details();
    active.question.answer = "Resume";
    expect(render(active, true)).toMatch(/<button[^>]*disabled/);
    if (active.recovery.kind === "job_control") active.recovery.control.continued_at = 10;
    expect(recoveryCommand(active)).toBeNull();
  });

  it("binds artwork recovery to the question and keeps exact approval separate", () => {
    const value = details({
      recovery: {
        kind: "artwork",
        decision: {
          question_id: 7,
          job_id: 3,
          plan_version: 2,
          issue: "missing",
          paused_state: "queued",
          candidate_id: 9,
          sha256: "a".repeat(64),
          resumed_at: null,
        },
      },
    });
    value.question.answer = "Use approved cover";
    expect(recoveryCommand(value)).toEqual({
      path: "/api/questions/7/artwork/resume",
      plan_version: 2,
    });
    expect(render(value)).toContain("Apply approved cover and restore safe state");
    expect(render(value)).toContain("approve the exact JPEG");
    if (value.recovery.kind === "artwork") value.recovery.decision.resumed_at = 10;
    expect(recoveryCommand(value)).toBeNull();
  });

  it("shows saved answers still waiting for recovery and escapes prompt text", () => {
    const question = details().question;
    question.prompt = "<script>not instructions</script>";
    question.answer = "Cancel";
    const markup = renderToStaticMarkup(
      <Inbox questions={[question]} loading={false} error={undefined} refresh={() => {}} />,
    );
    expect(markup).toContain("#/inbox/7");
    expect(markup).toContain("Saved answer: Cancel");
    expect(markup).toContain("Recovery action needed");
    expect(markup).toContain("&lt;script&gt;");
    expect(markup).not.toContain("<script>");
  });

  it("does not describe a failed inbox read as empty", () => {
    const markup = renderToStaticMarkup(
      <Inbox
        questions={[]}
        loading={false}
        error={new Error("State is unavailable")}
        refresh={() => {}}
      />,
    );
    expect(markup).toContain("State is unavailable");
    expect(markup).not.toContain("No requests are waiting");
  });
});
