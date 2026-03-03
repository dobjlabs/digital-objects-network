import { useMemo, useState } from "react";
import {
  attachClaim,
  createPost,
  respondPost,
  verifyPostProofs,
} from "../../shared/api/tauriClient";
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
  const [composeMode, setComposeMode] = useState<"closed" | "new" | "reply">(
    "closed",
  );
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
  const [verifiedProofMap, setVerifiedProofMap] = useState<
    Record<string, "live" | "nullified">
  >({});

  const toValidity = (value: string) =>
    value === "nullified" ? "nullified" : "live";
  const nowLabel = () => new Date().toLocaleString();
  const countProofs = (proofs: Array<{ validity: "live" | "nullified" }>) => ({
    live: proofs.filter((proof) => proof.validity === "live").length,
    nullified: proofs.filter((proof) => proof.validity === "nullified").length,
    total: proofs.length,
  });
  const proofKeyForPost = (
    postId: string,
    proof: { hash: string },
    index: number,
  ) => `post:${postId}:${proof.hash}:${index}`;
  const proofKeyForResponse = (
    responseId: string,
    proof: { hash: string },
    index: number,
  ) => `resp:${responseId}:${proof.hash}:${index}`;

  const renderProofTag = (config: {
    proof: FeedPost["proofs"][number];
    key: string;
    proofKey?: string;
    inPost?: boolean;
  }) => {
    const { proof, key, proofKey, inPost = false } = config;
    const verifiedState = proofKey ? verifiedProofMap[proofKey] : undefined;
    const isVerifying = proofKey ? verifyingProofKeys.includes(proofKey) : false;
    return (
      <span
        key={key}
        className={`proof-pill ${proof.validity} ${
          proof.validity === "nullified" ? "nullified-tag" : ""
        } ${isVerifying ? "verifying" : ""} ${
          verifiedState === "live" ? "verified-live" : ""
        } ${verifiedState === "nullified" ? "verified-null" : ""}`}
      >
        <span className={`check ${proof.validity}`}>
          {proof.validity === "live" ? "✓" : "✗"}
        </span>
        <span>{proof.name}</span>
        <span className="proof-tooltip">
          {proof.hash} · {proof.validity}
        </span>
        {inPost && proof.validity === "nullified" && (
          <span className="proof-note">· spent after post</span>
        )}
      </span>
    );
  };

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

  const allProofTypes = useMemo(
    () => Array.from(proofTypeCounts.keys()).sort(),
    [proofTypeCounts],
  );

  const filteredPosts = useMemo(() => {
    const q = search.trim().toLowerCase();
    return localPosts.filter((post) => {
      if (
        q &&
        !post.title.toLowerCase().includes(q) &&
        !post.desc.toLowerCase().includes(q)
      ) {
        return false;
      }
      if (
        liveOnly &&
        !post.proofs.every((proof) => proof.validity === "live")
      ) {
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
    ? (localPosts.find((post) => post.id === activePostId) ?? null)
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
        setVerifyingProofKeys((prev) =>
          prev.filter((key) => key !== entry.key),
        );
        setVerifiedProofMap((prev) => ({
          ...prev,
          [entry.key]: entry.validity,
        }));
      }
      const result = await verifyPostProofs(postId);
      setVerifyState({
        status: "done",
        checkedBlock: result.checkedBlock,
        error: null,
      });
    } catch (error) {
      setVerifyState({
        status: "error",
        checkedBlock: null,
        error: error instanceof Error ? error.message : "Verification failed",
      });
    }
  };

  const toggleType = (type: string) => {
    setActiveTypes((prev) =>
      prev.includes(type)
        ? prev.filter((value) => value !== type)
        : [...prev, type],
    );
  };

  const handleAttachClaim = async () => {
    const value = claimName.trim();
    if (!value) return;
    try {
      const claim = await attachClaim(
        value.endsWith(".dobj") ? value : `${value}.dobj`,
      );
      setComposeProofs((prev) => [
        ...prev,
        { ...claim, validity: toValidity(claim.validity) },
      ]);
      setClaimName("");
      setComposeError(null);
    } catch (error) {
      setComposeError(
        error instanceof Error ? error.message : "Failed to attach claim",
      );
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
      setComposeError(
        error instanceof Error ? error.message : "Failed to submit",
      );
    } finally {
      setComposeSubmitting(false);
    }
  };

  if (composeMode !== "closed") {
    const replyTo = replyToPostId
      ? (localPosts.find((post) => post.id === replyToPostId) ?? null)
      : null;
    const isReply = composeMode === "reply";

    return (
      <section className="feed-panel">
        <div className="feed-detail-header">
          <button
            type="button"
            className="feed-back-btn"
            onClick={resetCompose}
          >
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
            className="feed-search feed-compose-input"
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
          <button
            type="button"
            className="feed-attach-btn"
            onClick={handleAttachClaim}
          >
            Attach Claim
          </button>
        </div>

        <div className="feed-proof-row">
          {composeProofs.map((proof, index) => (
            renderProofTag({
              proof,
              key: `compose:${proof.hash}:${index}`,
            })
          ))}
        </div>

        {composeError && (
          <div className="feed-verify-error">{composeError}</div>
        )}

        <div className="feed-verify-bar">
          <button
            type="button"
            className="feed-compose-submit"
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
    const allProofs = [
      ...activePost.proofs,
      ...activePost.responses.flatMap((response) => response.proofs),
    ];
    const proofCounts = countProofs(allProofs);
    return (
      <section className="feed-panel">
        <div className="feed-detail-header">
          <button
            type="button"
            className="feed-back-btn"
            onClick={() => setActivePostId(null)}
          >
            ← back
          </button>
          <div className="feed-title">{activePost.title}</div>
        </div>
        <div className="feed-verify-summary">
          <span className="feed-verify-stat live">
            ✓ {proofCounts.live} live
          </span>
          {proofCounts.nullified > 0 && (
            <span className="feed-verify-stat nullified">
              ✗ {proofCounts.nullified} nullified
            </span>
          )}
          <span className="feed-verify-block">
            {verifyState.status === "done"
              ? `checked block #${verifyState.checkedBlock}`
              : verifyState.status === "running"
                ? "verifying..."
                : "unchecked"}
          </span>
          <button
            type="button"
            className="feed-verify-btn"
            disabled={verifyState.status === "running"}
            onClick={() => handleVerify(activePost.id)}
          >
            {verifyState.status === "running" ? "verifying..." : "verify all"}
          </button>
        </div>
        <div className="feed-meta">
          {activePost.time} · {activePost.peer}
        </div>
        <div className="feed-proof-row">
          {activePost.proofs.map((proof, index) => (
            renderProofTag({
              proof,
              key: `postproof:${proof.hash}:${index}`,
              proofKey: proofKeyForPost(activePost.id, proof, index),
              inPost: true,
            })
          ))}
        </div>
        <p className="feed-desc">{activePost.desc}</p>
        <div className="feed-responses">
          <div className="feed-response-count">
            {activePost.responses.length} response
            {activePost.responses.length === 1 ? "" : "s"}
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
                  renderProofTag({
                    proof,
                    key: `resp:${response.id}:${proof.hash}:${index}`,
                    proofKey: proofKeyForResponse(response.id, proof, index),
                    inPost: true,
                  })
                ))}
              </div>
              <div className="feed-response-desc">{response.desc}</div>
            </div>
          ))}
        </div>
        <div className="feed-verify-bar">
          <button
            type="button"
            className="feed-respond-btn"
            onClick={() => {
              setComposeMode("reply");
              setReplyToPostId(activePost.id);
              setComposeDesc("");
              setComposeProofs([]);
              setComposeError(null);
            }}
          >
            ↩ Respond
          </button>
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
          className="feed-post-btn"
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
              {type}{" "}
              <span className="feed-chip-count">
                {proofTypeCounts.get(type) ?? 0}
              </span>
            </button>
          ))}
        </div>
      )}
      <div className="feed-list">
        {filteredPosts.length === 0 && (
          <div className="feed-empty">No posts match.</div>
        )}
        {filteredPosts.map((post) => (
          <button
            key={post.id}
            type="button"
            className="feed-item"
            onClick={() => {
              setVerifyState({
                status: "idle",
                checkedBlock: null,
                error: null,
              });
              setActivePostId(post.id);
            }}
          >
            <div className="feed-item-row1">
              <div className="feed-item-title">
                {post.title}
                {post.responses.length > 0 && (
                  <span className="feed-item-replies">
                    {post.responses.length} repl
                    {post.responses.length === 1 ? "y" : "ies"}
                  </span>
                )}
              </div>
              <div className="feed-item-time">
                {post.time} · {post.peer}
              </div>
            </div>
            <div className="feed-proof-row feed-proof-row-list">
              {post.proofs.map((proof, index) =>
                renderProofTag({
                  proof,
                  key: `list:${post.id}:${proof.hash}:${index}`,
                }),
              )}
            </div>
          </button>
        ))}
      </div>
    </section>
  );
}
