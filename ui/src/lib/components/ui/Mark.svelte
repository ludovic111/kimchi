<script lang="ts">
  // The kimchi mark: a napa stem with a chili leaf and a green-onion leaf.
  let { size = 20, mono = false }: { size?: number; mono?: boolean } = $props();
</script>

<svg width={size} height={size} viewBox="0 0 64 64" fill="none" aria-hidden="true">
  <rect x="14" y="10" width="8" height="44" rx="4" fill={mono ? "currentColor" : "#FFF4E6"} />
  <path d="M23.5 32.5c1.6-10 9.4-17 21.5-19.3-1.2 11.9-9 18.6-21.5 19.3z" fill={mono ? "currentColor" : "#FF5A36"} />
  <path d="M23.5 35.5c12.8.6 19.8 5.7 22.1 14.2-10-.3-18.6-4.9-22.1-14.2z" fill={mono ? "currentColor" : "#A6CF5E"} />
</svg>
