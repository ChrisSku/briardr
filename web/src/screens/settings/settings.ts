// Settings as the host reports them, and how sections save changes.
export type { Settings, UpdateStatus } from '../../protocol.ts';

export type Save = (patch: object) => Promise<boolean>;
