-- Optional category label on audiobook for left-pane grouping in the
-- library view. Free-text (max 60 chars) — admins/users supply whatever
-- bucketing makes sense ("Bedtime stories", "Tech talks", "Marketing").
-- `genre` already exists for the AI-content type; this is the user's
-- own grouping, surfaced in the master list as collapsible sections.

DEFINE FIELD IF NOT EXISTS category ON audiobook TYPE option<string>
    ASSERT $value = NONE OR string::len($value) <= 60;
