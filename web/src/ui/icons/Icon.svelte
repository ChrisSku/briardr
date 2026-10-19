<script lang="ts">
  import { SHAPES, type IconName, type IconProps } from './shapes.ts';

  let { name, size = 20, label, class: cls }: IconProps & { name: IconName } = $props();
</script>

<svg
  class={['icon', cls]}
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.8"
  stroke-linecap="round"
  stroke-linejoin="round"
  role={label ? 'img' : undefined}
  aria-label={label}
  aria-hidden={label ? undefined : 'true'}
  focusable="false"
>
  {@html SHAPES[name]}
</svg>
