import { useMemo, useState } from "react";
import { verifyPostProofs } from "../../shared/api/tauriClient";
import type { FeedPost } from "../../shared/types/domain";

interface FeedPanelProps {
  posts: FeedPost[];
}

export function FeedPanel({ posts }: FeedPanelProps) {
  const [search, setSearch] = useState("");
  const [activePostId, setActivePostId] = useState<string | null>(null);
  const [verifyState, setVerifyState] = useState<{
    status: "idle" | "running" | "done" | "error";
    checkedBlock: string | null;
    error: string | null;
  }>({ status: "idle", checkedBlock: null, error: null });

  const filteredPosts = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return posts;
    return posts.filter(
      (post) => post.title.toLowerCase().includes(q) || post.desc.toLowerCase().includes(q),
    );
  }, [posts, search]);

  const activePost = activePostId ? posts.find((post) => post.id === activePostId) ?? null : null;

  const handleVerify = async (postId: string) => {
    setVerifyState({ status: "running", checkedBlock: null, error: null });
    try {
      const result = await verifyPostProofs(postId);
      setVerifyState({ status: "done", checkedBlock: result.checkedBlock, error: null });
    } catch (error) {
      setVerifyState({
        status: "error",
        checkedBlock: null,
        error: error instanceof Error ? error.message : "Verification failed",
      });
    }
  };

  if (activePost) {
    return (
      <section className="feed-panel">
        <div className="feed-detail-header">
          <button type="button" className="feed-back-btn" onClick={() => setActivePostId(null)}>
            ← back
          </button>
          <div className="feed-title">{activePost.title}</div>
        </div>
        <div className="feed-meta">
          {activePost.time} · {activePost.peer}
        </div>
        <div className="feed-proof-row">
          {activePost.proofs.map((proof, index) => (
            <span key={`${proof.hash}-${index}`} className={`proof-pill ${proof.validity}`}>
              {proof.validity === "live" ? "✓" : "✗"} {proof.name}
            </span>
          ))}
        </div>
        <p className="feed-desc">{activePost.desc}</p>
        <div className="feed-verify-bar">
          <button
            type="button"
            className="feed-verify-btn"
            disabled={verifyState.status === "running"}
            onClick={() => handleVerify(activePost.id)}
          >
            {verifyState.status === "running" ? "verifying..." : "verify all"}
          </button>
          {verifyState.status === "done" && (
            <span className="feed-verify-msg">checked block #{verifyState.checkedBlock}</span>
          )}
          {verifyState.status === "error" && (
            <span className="feed-verify-error">{verifyState.error}</span>
          )}
        </div>
      </section>
    );
  }

  return (
    <section className="feed-panel">
      <div className="feed-toolbar">
        <input
          className="feed-search"
          placeholder="search posts..."
          value={search}
          onChange={(event) => setSearch(event.target.value)}
        />
      </div>
      <div className="feed-list">
        {filteredPosts.length === 0 && <div className="feed-empty">No posts match.</div>}
        {filteredPosts.map((post) => (
          <button
            key={post.id}
            type="button"
            className="feed-item"
            onClick={() => {
              setVerifyState({ status: "idle", checkedBlock: null, error: null });
              setActivePostId(post.id);
            }}
          >
            <div className="feed-item-title">{post.title}</div>
            <div className="feed-item-meta">
              {post.time} · {post.peer}
            </div>
          </button>
        ))}
      </div>
    </section>
  );
}
