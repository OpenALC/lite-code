export interface CoreHandle {
  id: string;
  native: boolean;
}

export const createHandle: () => CoreHandle;
