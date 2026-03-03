import { useMemo, useState } from "react";
import { attachClaim, createPost, respondPost, verifyPostProofs } from "../../shared/api/tauriClient";
import type { FeedPost } from "../../shared/types/domain";

interface FeedPanelProps {
  posts: FeedPost[];
}

export function FeedPanel({ posts }: FeedPanelProps) {
  const [localPosts, setLocalPosts] = useState<FeedPost[]>(posts);
  const [search, setSearch] = useState("");
  const [liveOnly, setLiveOnly] = useState(false);
  const [filterOpen, setFilterOpen] = useState(false);
  const [activeTypes, setActiveTypes] = useState<string[]>([]);
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
  const [verifyingProofKeys, setVerifyingProofKeys] = useState<string[]>([]);
  const [verifiedProofMap, setVerifiedProofMap] = useState<Record<string, "live" | "nullified">>(
    {},
  );

  const toValidity = (value: string) => (value === "nullified" ? "nullified" : "live");
  const nowLabel = () => new Date().toLocaleString();

  const proofTypeCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const post of localPosts) {
      const uniqueInPost = new Set(post.proofs.map((proof) => proof.name));
      for (const type of uniqueInPost) {
        counts.set(type, (counts.get(type) ?? 0) + 1);
      }
    }
    return counts;
  }, [localPosts]);

  const allProofTypes = useMemo(() => Array.from(proofTypeCounts.keys()).sort(), [proofTypeCounts]);

  const filteredPosts = useMemo(() => {
    const q = search.trim().toLowerCase();
    return localPosts.filter((post) => {
      if (q && !post.title.toLowerCase().includes(q) && !post.desc.toLowerCase().includes(q)) {
        return false;
      }
      if (liveOnly && !post.proofs.every((proof) => proof.validity === "live")) {
        return false;
      }
      if (
        activeTypes.length > 0 &&
        !post.proofs.some((proof) => activeTypes.includes(proof.name))
      ) {
        return false;
      }
      return true;
    });
  }, [localPosts, search, liveOnly, activeTypes]);

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
    const target = localPosts.find((post) => post.id === postId);
    if (!target) return;

    const proofEntries = [
      ...target.proofs.map((proof, index) => ({
        key: `post:${postId}:${proof.hash}:${index}`,
        validity: proof.validity,
      })),
      ...target.responses.flatMap((response) =>
        response.proofs.map((proof, index) => ({
          key: `resp:${response.id}:${proof.hash}:${index}`,
          validity: proof.validity,
        })),
      ),
    ];

    setVerifyState({ status: "running", checkedBlock: null, error: null });
    setVerifiedProofMap({});
    setVerifyingProofKeys([]);
    try {
      for (const entry of proofEntries) {
        setVerifyingProofKeys((prev) => [...prev, entry.key]);
        await new Promise((resolve) => setTimeout(resolve, 220));
        setVerifyingProofKeys((prev) => prev.filter((key) => key !== entry.key));
        setVerifiedProofMap((prev) => ({
          ...prev,
          [entry.key]: entry.validity,
        }));
      }
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

  const toggleType = (type: string) => {
    setActiveTypes((prev) => (prev.includes(type) ? prev.filter((value) => value !== type) : [...prev, type]));
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
        setLocalPosts((prev) =>
          prev.map((post) => {
            if (post.id !== replyToPostId) return post;
            return {
              ...post,
              responses: [
                ...post.responses,
                {
                  id: `resp-${Date.now()}`,
                  peer: "127.0.0.1",
                  time: nowLabel(),
                  desc,
                  proofs: [...composeProofs],
                },
              ],
            };
          }),
        );
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
            <span
              key={`${proof.hash}-${index}`}
              className={`proof-pill ${proof.validity} ${
                verifyingProofKeys.includes(`post:${activePost.id}:${proof.hash}:${index}`)
                  ? "verifying"
                  : ""
              } ${
                verifiedProofMap[`post:${activePost.id}:${proof.hash}:${index}`] === "live"
                  ? "verified-live"
                  : ""
              } ${
                verifiedProofMap[`post:${activePost.id}:${proof.hash}:${index}`] === "nullified"
                  ? "verified-null"
                  : ""
              }`}
            >
              {proof.validity === "live" ? "✓" : "✗"} {proof.name}
            </span>
          ))}
        </div>
        <p className="feed-desc">{activePost.desc}</p>
        <div className="feed-responses">
          <div className="feed-response-count">
            {activePost.responses.length} response{activePost.responses.length === 1 ? "" : "s"}
          </div>
          {activePost.responses.length === 0 && (
            <div className="feed-empty">No responses yet.</div>
          )}
          {activePost.responses.map((response) => (
            <div key={response.id} className="feed-response-item">
              <div className="feed-item-meta">
                {response.time} · {response.peer}
              </div>
              <div className="feed-proof-row">
                {response.proofs.map((proof, index) => (
                  <span
                    key={`${response.id}-${proof.hash}-${index}`}
                    className={`proof-pill ${proof.validity} ${
                      verifyingProofKeys.includes(`resp:${response.id}:${proof.hash}:${index}`)
                        ? "verifying"
                        : ""
                    } ${
                      verifiedProofMap[`resp:${response.id}:${proof.hash}:${index}`] === "live"
                        ? "verified-live"
                        : ""
                    } ${
                      verifiedProofMap[`resp:${response.id}:${proof.hash}:${index}`] === "nullified"
                        ? "verified-null"
                        : ""
                    }`}
                  >
                    {proof.validity === "live" ? "✓" : "✗"} {proof.name}
                  </span>
                ))}
              </div>
              <div className="feed-response-desc">{response.desc}</div>
            </div>
          ))}
        </div>
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
        <label className="feed-live-toggle">
          <input
            type="checkbox"
            checked={liveOnly}
            onChange={(event) => setLiveOnly(event.target.checked)}
          />
          Live only
        </label>
        <button
          type="button"
          className={`feed-filter-btn ${filterOpen ? "active" : ""}`}
          onClick={() => setFilterOpen((prev) => !prev)}
        >
          Filter ▾
        </button>
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
      {filterOpen && (
        <div className="feed-filter-chips">
          {allProofTypes.map((type) => (
            <button
              key={type}
              type="button"
              className={`feed-chip ${activeTypes.includes(type) ? "active" : ""}`}
              onClick={() => toggleType(type)}
            >
              {type} <span className="feed-chip-count">{proofTypeCounts.get(type) ?? 0}</span>
            </button>
          ))}
        </div>
      )}
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
