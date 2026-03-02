import { useMemo, useState } from "react";
import { attachClaim, createPost, respondPost, verifyPostProofs } from "../../shared/api/tauriClient";
import type { FeedPost } from "../../shared/types/domain";

interface FeedPanelProps {
  posts: FeedPost[];
}

export function FeedPanel({ posts }: FeedPanelProps) {
  const [localPosts, setLocalPosts] = useState<FeedPost[]>(posts);
  const [search, setSearch] = useState("");
  const [activePostId, setActivePostId] = useState<string | null>(null);
  const [composeMode, setComposeMode] = useState<"closed" | "new" | "reply">("closed");
  const [replyToPostId, setReplyToPostId] = useState<string | null>(null);
  const [composeTitle, setComposeTitle] = useState("");
  const [composeDesc, setComposeDesc] = useState("");
  const [claimName, setClaimName] = useState("");
  const [composeProofs, setComposeProofs] = useState<FeedPost["proofs"]>([]);
  const [composeError, setComposeError] = useState<string | null>(null);
  const [composeSubmitting, setComposeSubmitting] = useState(false);
  const [verifyState, setVerifyState] = useState<{
    status: "idle" | "running" | "done" | "error";
    checkedBlock: string | null;
    error: string | null;
  }>({ status: "idle", checkedBlock: null, error: null });

  const toValidity = (value: string) => (value === "nullified" ? "nullified" : "live");

  const filteredPosts = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return localPosts;
    return localPosts.filter(
      (post) => post.title.toLowerCase().includes(q) || post.desc.toLowerCase().includes(q),
    );
  }, [localPosts, search]);

  const activePost = activePostId
    ? localPosts.find((post) => post.id === activePostId) ?? null
    : null;

  const resetCompose = () => {
    setComposeMode("closed");
    setReplyToPostId(null);
    setComposeTitle("");
    setComposeDesc("");
    setClaimName("");
    setComposeProofs([]);
    setComposeError(null);
    setComposeSubmitting(false);
  };

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

  const handleAttachClaim = async () => {
    const value = claimName.trim();
    if (!value) return;
    try {
      const claim = await attachClaim(value.endsWith(".dobj") ? value : `${value}.dobj`);
      setComposeProofs((prev) => [...prev, { ...claim, validity: toValidity(claim.validity) }]);
      setClaimName("");
      setComposeError(null);
    } catch (error) {
      setComposeError(error instanceof Error ? error.message : "Failed to attach claim");
    }
  };

  const handleSubmitCompose = async () => {
    const proofNames = composeProofs.map((proof) => proof.name);
    const desc = composeDesc.trim();
    if (!desc) {
      setComposeError("Description is required.");
      return;
    }

    setComposeSubmitting(true);
    setComposeError(null);

    try {
      if (composeMode === "new") {
        const title = composeTitle.trim();
        if (!title) {
          setComposeError("Title is required for new posts.");
          setComposeSubmitting(false);
          return;
        }
        const created = await createPost({ title, desc, proofNames });
        setLocalPosts((prev) => [
          {
            id: created.id,
            title: created.title,
            peer: created.peer,
            time: created.time,
            desc: created.desc,
            proofs: created.proofs.map((proof) => ({
              ...proof,
              validity: toValidity(proof.validity),
            })),
            responses: [],
          },
          ...prev,
        ]);
        resetCompose();
        return;
      }

      if (composeMode === "reply" && replyToPostId) {
        await respondPost({
          postId: replyToPostId,
          desc,
          proofNames,
        });
        resetCompose();
      }
    } catch (error) {
      setComposeError(error instanceof Error ? error.message : "Failed to submit");
    } finally {
      setComposeSubmitting(false);
    }
  };

  if (composeMode !== "closed") {
    const replyTo = replyToPostId
      ? localPosts.find((post) => post.id === replyToPostId) ?? null
      : null;
    const isReply = composeMode === "reply";

    return (
      <section className="feed-panel">
        <div className="feed-detail-header">
          <button type="button" className="feed-back-btn" onClick={resetCompose}>
            ← back
          </button>
          <div className="feed-title">{isReply ? "Respond" : "New Post"}</div>
        </div>

        {isReply && replyTo && (
          <div className="feed-compose-context">
            <div className="feed-compose-context-title">{replyTo.title}</div>
          </div>
        )}

        {!isReply && (
          <input
            className="feed-search"
            placeholder="Title"
            value={composeTitle}
            onChange={(event) => setComposeTitle(event.target.value)}
          />
        )}

        <textarea
          className="feed-compose-textarea"
          placeholder="Description"
          value={composeDesc}
          onChange={(event) => setComposeDesc(event.target.value)}
        />

        <div className="feed-compose-claims">
          <input
            className="feed-search"
            placeholder="claim file (e.g. Asteroid.dobj)"
            value={claimName}
            onChange={(event) => setClaimName(event.target.value)}
          />
          <button type="button" className="feed-back-btn" onClick={handleAttachClaim}>
            Attach Claim
          </button>
        </div>

        <div className="feed-proof-row">
          {composeProofs.map((proof, index) => (
            <span key={`${proof.hash}-${index}`} className={`proof-pill ${proof.validity}`}>
              {proof.validity === "live" ? "✓" : "✗"} {proof.name}
            </span>
          ))}
        </div>

        {composeError && <div className="feed-verify-error">{composeError}</div>}

        <div className="feed-verify-bar">
          <button
            type="button"
            className="feed-verify-btn"
            onClick={handleSubmitCompose}
            disabled={composeSubmitting}
          >
            {composeSubmitting ? "Submitting..." : "Submit"}
          </button>
        </div>
      </section>
    );
  }

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
            className="feed-back-btn"
            onClick={() => {
              setComposeMode("reply");
              setReplyToPostId(activePost.id);
              setComposeDesc("");
              setComposeProofs([]);
              setComposeError(null);
            }}
          >
            respond
          </button>
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
        <button
          type="button"
          className="feed-verify-btn"
          onClick={() => {
            setComposeMode("new");
            setComposeTitle("");
            setComposeDesc("");
            setComposeProofs([]);
            setComposeError(null);
          }}
        >
          + Post
        </button>
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
