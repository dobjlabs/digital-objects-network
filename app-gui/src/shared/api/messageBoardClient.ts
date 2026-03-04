export type ClaimValidity = "live" | "nullified";

export interface MessageBoardClaim {
  name: string;
  validity: ClaimValidity;
  hash: string;
}

export interface MessageBoardResponse {
  id: string;
  postId: string;
  peer: string;
  time: string;
  desc: string;
  proofs: MessageBoardClaim[];
}

export interface MessageBoardPost {
  id: string;
  title: string;
  peer: string;
  time: string;
  description: string;
  proofs: MessageBoardClaim[];
  responses: MessageBoardResponse[];
}

export interface ListPostsResponse {
  items: MessageBoardPost[];
  nextCursor: string | null;
}

export interface ListPostsParams {
  limit?: number;
  cursor?: string;
  q?: string;
  liveOnly?: boolean;
}

export interface CreatePostInput {
  title: string;
  description: string;
  claims: MessageBoardClaim[];
}

export interface CreateResponseInput {
  description: string;
  claims: MessageBoardClaim[];
}

const DEFAULT_BASE_URL = "http://127.0.0.1:3100";

function baseUrl() {
  const env = import.meta.env.VITE_MESSAGE_BOARD_BASE_URL;
  return typeof env === "string" && env.trim().length > 0
    ? env.trim().replace(/\/$/, "")
    : DEFAULT_BASE_URL;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${baseUrl()}${path}`, {
    headers: {
      "content-type": "application/json",
      ...(init?.headers ?? {}),
    },
    ...init,
  });

  if (!response.ok) {
    let message = `Request failed (${response.status})`;
    try {
      const payload = (await response.json()) as { error?: string };
      if (payload.error) message = payload.error;
    } catch {
      // Keep default message when body is not JSON.
    }
    throw new Error(message);
  }

  return (await response.json()) as T;
}

export function listPosts(params: ListPostsParams = {}): Promise<ListPostsResponse> {
  const query = new URLSearchParams();
  if (typeof params.limit === "number") query.set("limit", String(params.limit));
  if (params.cursor) query.set("cursor", params.cursor);
  if (params.q) query.set("q", params.q);
  if (typeof params.liveOnly === "boolean") {
    query.set("liveOnly", String(params.liveOnly));
  }

  const suffix = query.toString();
  const path = suffix ? `/api/v1/posts?${suffix}` : "/api/v1/posts";
  return request<ListPostsResponse>(path);
}

export function createPost(input: CreatePostInput): Promise<MessageBoardPost> {
  return request<MessageBoardPost>("/api/v1/posts", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export function createResponse(
  postId: string,
  input: CreateResponseInput,
): Promise<MessageBoardResponse> {
  return request<MessageBoardResponse>(`/api/v1/posts/${postId}/responses`, {
    method: "POST",
    body: JSON.stringify(input),
  });
}
