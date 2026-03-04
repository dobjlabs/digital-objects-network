CREATE TABLE IF NOT EXISTS posts (
  id UUID PRIMARY KEY,
  title TEXT NOT NULL,
  description TEXT NOT NULL,
  author_ip INET NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS post_claims (
  id UUID PRIMARY KEY,
  post_id UUID NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  validity TEXT NOT NULL CHECK (validity IN ('live', 'nullified')),
  hash TEXT NOT NULL,
  position INT NOT NULL
);

CREATE TABLE IF NOT EXISTS responses (
  id UUID PRIMARY KEY,
  post_id UUID NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
  description TEXT NOT NULL,
  author_ip INET NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS response_claims (
  id UUID PRIMARY KEY,
  response_id UUID NOT NULL REFERENCES responses(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  validity TEXT NOT NULL CHECK (validity IN ('live', 'nullified')),
  hash TEXT NOT NULL,
  position INT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_posts_created_id_desc
  ON posts (created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_responses_post_created_id_asc
  ON responses (post_id, created_at ASC, id ASC);
