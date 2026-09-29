/** Shared brand artwork; adjacent text supplies the accessible name. */
export function MakoMark({ small = false }: { readonly small?: boolean }) {
  return (
    <img
      src="/assets/mako-cloud.png"
      alt=""
      aria-hidden="true"
      width={small ? 28 : 40}
      height={small ? 28 : 40}
      className={`${small ? "size-7 rounded-lg" : "size-10 rounded-xl"} shrink-0 object-contain bg-white`}
    />
  );
}
