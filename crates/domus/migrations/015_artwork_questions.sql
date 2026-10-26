CREATE TABLE artwork_questions (
    question_id INTEGER PRIMARY KEY REFERENCES questions(id),
    issue TEXT NOT NULL CHECK(issue IN
      ('missing','permission','preview_approval','background','animated','color_profile','unsupported'))
) STRICT;
CREATE TABLE artwork_question_resolutions (
    question_id INTEGER PRIMARY KEY REFERENCES artwork_questions(question_id),
    candidate_id INTEGER NOT NULL REFERENCES artwork_candidates(id),
    sha256 TEXT NOT NULL CHECK(length(sha256)=64),
    resumed_at INTEGER CHECK(resumed_at IS NULL OR resumed_at > 0)
) STRICT;
CREATE TRIGGER immutable_artwork_question BEFORE UPDATE ON artwork_questions
WHEN NEW.question_id != OLD.question_id
  OR EXISTS(SELECT 1 FROM questions WHERE id=OLD.question_id AND answer IS NOT NULL)
BEGIN SELECT RAISE(ABORT,'artwork question is immutable'); END;
CREATE TRIGGER retained_artwork_question BEFORE DELETE ON artwork_questions
BEGIN SELECT RAISE(ABORT,'artwork question is retained'); END;
CREATE TRIGGER immutable_artwork_resolution BEFORE UPDATE ON artwork_question_resolutions
WHEN NEW.question_id != OLD.question_id OR NEW.candidate_id != OLD.candidate_id
  OR NEW.sha256 != OLD.sha256 OR OLD.resumed_at IS NOT NULL OR NEW.resumed_at IS NULL
BEGIN SELECT RAISE(ABORT,'artwork resolution is immutable'); END;
CREATE TRIGGER retained_artwork_resolution BEFORE DELETE ON artwork_question_resolutions
BEGIN SELECT RAISE(ABORT,'artwork resolution is retained'); END;
