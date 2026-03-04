import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface MockState {
  postCount: number;
  supportedMethods: string[];
}

export interface CreateDobjInput {
  dobjId: string;
  inputFiles: string[];
}

export interface CreateDobjResult {
  ok: boolean;
  oldRoot: string;
  newRoot: string;
  outputFile: string;
}

export interface CreateDobjProgress {
  dobjId: string;
  phase: "hash" | "verify" | "nullify" | "commit";
  status: "running" | "done";
  message: string;
  verifyIndex: number | null;
  detail: string | null;
  oldRoot: string | null;
  newRoot: string | null;
  outputFile: string | null;
}

export interface AttachClaimResult {
  name: string;
  validity: string;
  hash: string;
}

export interface CpuSample {
  usagePct: number;
  totalCpuSecs: number;
}

export function getThingsDir(): Promise<string> {
  return invoke<string>("get_things_dir");
}

export function openThingsDir(): Promise<string> {
  return invoke<string>("open_things_dir");
}

export function getMockState(): Promise<MockState> {
  return invoke<MockState>("get_mock_state");
}

export function createDobj(input: CreateDobjInput): Promise<CreateDobjResult> {
  return invoke<CreateDobjResult>("create_dobj", { input });
}

export function listenCreateDobjProgress(
  handler: (event: CreateDobjProgress) => void,
): Promise<UnlistenFn> {
  return listen<CreateDobjProgress>("create-dobj-progress", (event) => {
    handler(event.payload);
  });
}

export function attachClaim(fileName: string): Promise<AttachClaimResult> {
  return invoke<AttachClaimResult>("attach_claim", { input: { fileName } });
}

export function sampleAppCpu(): Promise<CpuSample> {
  return invoke<CpuSample>("sample_app_cpu");
}
