export async function listen<T>(_name: string, _cb: (e: { payload: T }) => void): Promise<() => void> {
  return () => {};
}
export type UnlistenFn = () => void;
