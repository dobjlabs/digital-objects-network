import { invoke } from "@tauri-apps/api/core";

export interface MockStateDto {
  postCount: number;
  supportedMethods: string[];
}

export interface RunMethodInput {
  methodName: string;
  args: string[];
  cpuCost: string;
}

export interface ProofRunResult {
  success: boolean;
  methodName: string;
  oldRoot: string;
  newRoot: string;
  stageMessages: string[];
}

export interface VerifyResult {
  postId: string;
  status: string;
  checkedBlock: string;
}

export interface CreatePostInput {
  title: string;
  desc: string;
  proofNames: string[];
}

export interface ProofClaimDto {
  name: string;
  validity: string;
  hash: string;
}

export interface PostDto {
  id: string;
  title: string;
  peer: string;
  time: string;
  desc: string;
  proofs: ProofClaimDto[];
}

export interface RespondPostInput {
  postId: string;
  desc: string;
  proofNames: string[];
}

export interface GenericActionResult {
  ok: boolean;
  message: string;
}

export interface AttachClaimResult {
  name: string;
  validity: string;
  hash: string;
}

export interface CpuSampleDto {
  usagePct: number;
  totalCpuSecs: number;
}

export function getThingsDir(): Promise<string> {
  return invoke<string>("get_things_dir");
}

export function openThingsDir(): Promise<string> {
  return invoke<string>("open_things_dir");
}

export function getMockState(): Promise<MockStateDto> {
  return invoke<MockStateDto>("get_mock_state");
}

export function runMethod(input: RunMethodInput): Promise<ProofRunResult> {
  return invoke<ProofRunResult>("run_method", { input });
}

export function verifyPostProofs(postId: string): Promise<VerifyResult> {
  return invoke<VerifyResult>("verify_post_proofs", { input: { postId } });
}

export function createPost(input: CreatePostInput): Promise<PostDto> {
  return invoke<PostDto>("create_post", { input });
}

export function respondPost(
  input: RespondPostInput,
): Promise<GenericActionResult> {
  return invoke<GenericActionResult>("respond_post", { input });
}

export function attachClaim(fileName: string): Promise<AttachClaimResult> {
  return invoke<AttachClaimResult>("attach_claim", { input: { fileName } });
}

export function sampleAppCpu(): Promise<CpuSampleDto> {
  return invoke<CpuSampleDto>("sample_app_cpu");
}
