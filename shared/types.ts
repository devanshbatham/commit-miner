export interface ScanOptions {
  source: string;
  commit?: string;
  limit?: number;
  since?: string;
  until?: string;
  firstParent: boolean;
  threshold: number;
  concurrency: number;
  cache: boolean;
}
export interface Commit {
  sha: string; parents: string[]; date: string; author: string; message: string;
  committedAt?: string; files: string[]; merge: boolean; excludedFiles?: number;
}
export interface DiffLine {
  kind: 'added' | 'removed' | 'context' | 'meta';
  text: string; oldLine: number | null; newLine: number | null;
}
export interface Evidence {
  id: string; sha: string; path: string; header: string; lines: DiffLine[];
  part: number; parts: number;
}
export interface Category { id: string; label: string; family: 'bug' | 'security' | 'change'; description: string; cwe?: number }
export interface SegmentResult {
  evidence: Evidence; probabilities: Record<string, number>; cached: boolean; model: string;
}
export interface CommitResult {
  commit: Commit; status: 'evaluating' | 'complete' | 'partial' | 'pending' | 'failed';
  evaluated: number; total: number; probabilities: Record<string, number>;
  categories: { id: string; probability: number }[];
  evidence: SegmentResult[]; warnings: string[];
  aggregation: 'commit_review';
  reviewCoverage: 'full' | 'selected' | 'metadata' | 'unavailable';
}
export interface CostEstimate {
  models: Record<string, {
    inputTokens: number; outputTokens: number;
    price: { inputPerMillionUsd: number; outputPerMillionUsd: number; checkedOn: string } | null;
  }>;
}
export interface Progress {
  cost?: CostEstimate | null;
  phase: string; done: number; total: number; percent: number; unit?: 'files' | 'commits'; files?:number; highPriority?:number;
  commits: number; classified: number; calls: number; cached: number; skipped: number;
  failed?:number; metadataOnly?:number; excludedFiles?: number; excludedChanges?: number; excludedCommits?: number;
  bugs: number; security: number; requestsPerSecond: number; elapsedSeconds?:number; active: number; sections?:number; reviewedSections?:number; inputTokens?:number; outputTokens?:number;
}
export interface ScanEvent { id: number; type: string; time: string; data: any }
export interface ScanSummary { id: string; status: string; options: ScanOptions; started: string; progress: Progress }

export interface SavedScan {selection?:{only:string[];cwe:number[];minProbability:number;matched:number;classified:number};summary:ScanSummary;results:CommitResult[];events:ScanEvent[];ownerPid:number;error?:string}
