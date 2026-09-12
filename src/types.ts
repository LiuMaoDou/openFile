export interface ScopeInput {
  contentEnabled: boolean;
  path: string;
  recursive: boolean;
  watch: boolean;
  excludes: string[];
}
export interface Scope extends ScopeInput {
  contentPaused: boolean;
  id: string;
  name: string;
  availability: string;
  freshness: string;
  count: number;
  scanned: number;
  lastScan: number | null;
  message: string | null;
}
export interface Snippet {
  before: string;
  matched: string;
  after: string;
  truncated: boolean;
}
export interface Entry {
  snippet?: Snippet | null;
  contentReady?: boolean;
  id: string;
  name: string;
  directory: string;
  path: string;
  extension: string;
  group: string;
  size: number;
  mtime: number;
  scopeId: string;
  scopeName: string;
  online: boolean;
  hidden: boolean;
  placeholder: boolean;
}
export interface Bucket {
  name: string;
  count: number;
}
export interface Summary {
  facetScopeId: string;
  facetHidden: boolean;
  scanPaused: boolean;
  scopes: Scope[];
  total: number;
  hidden: number;
  size: number;
  extensions: Bucket[];
  hiddenExtensions: Bucket[];
  groups: Bucket[];
  revision: number;
}
export interface Query {
  searchMode: "name" | "content" | "all";
  search: string;
  scopeId: string;
  extension: string;
  group: string;
  hidden: boolean;
  minSize?: number;
  modifiedAfter?: number;
  sort: string;
  descending: boolean;
  offset: number;
  limit: number;
}
export interface QueryResult {
  searchEngine?: "local" | "everything" | "content";
  searchNotice?: string | null;
  entries: Entry[];
  total: number;
  revision: number;
  offset: number;
}
export interface DeleteItem {
  id: string;
  name: string;
  path: string;
  size: number;
  status:
    | "ready"
    | "blocked"
    | "running"
    | "succeeded"
    | "failed"
    | "unknown"
    | "cancelled";
  message: string | null;
}
export interface DeletePlan {
  destination?: string;
  id: string;
  created: number;
  expires: number;
  state: "planned" | "running" | "finished" | "cancelled" | "interrupted";
  items: DeleteItem[];
}
export const DEFAULT_QUERY: Query = {
  searchMode: "name",
  search: "",
  scopeId: "",
  extension: "",
  group: "",
  hidden: false,
  sort: "name",
  descending: false,
  offset: 0,
  limit: 200,
};
export const DEFAULT_EXCLUDES = [
  "node_modules",
  ".git",
  "$Recycle.Bin",
  "System Volume Information",
  "Windows/WinSxS",
  "*.tmp",
];
export const EMPTY_SUMMARY: Summary = {
  facetScopeId: "",
  facetHidden: false,
  scanPaused: false,
  scopes: [],
  total: 0,
  hidden: 0,
  size: 0,
  extensions: [],
  hiddenExtensions: [],
  groups: [],
  revision: 0,
};
