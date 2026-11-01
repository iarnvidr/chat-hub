export function createHoverPreview({
  delay = 220,
  exitGrace = 180,
  schedule = (fn, ms) => setTimeout(fn, ms),
  cancel = id => clearTimeout(id),
  load,
  show,
  hide,
}) {
  let enterTimer = null;
  let exitTimer = null;
  let generation = 0;
  let destroyed = false;
  let active = null;
  let pointerInPreview = false;
  let focusInPreview = false;
  const unbind = new Set();
  const boundPreviews = new WeakSet();

  const listen = (target, type, fn) => {
    target.addEventListener(type, fn);
    const off = () => { target.removeEventListener(type, fn); unbind.delete(off); };
    unbind.add(off);
    return off;
  };
  const clear = name => {
    if (name === 'enter' && enterTimer !== null) cancel(enterTimer);
    if (name === 'exit' && exitTimer !== null) cancel(exitTimer);
    if (name === 'enter') enterTimer = null;
    if (name === 'exit') exitTimer = null;
  };
  const engaged = binding => !!binding && (binding.pointer || binding.focus || pointerInPreview || focusInPreview);
  const close = () => {
    const hadActive = active !== null;
    clear('enter'); clear('exit');
    generation++;
    active = null;
    pointerInPreview = false;
    focusInPreview = false;
    if (hadActive) hide?.();
  };
  const scheduleHide = () => {
    clear('exit');
    const token = generation;
    exitTimer = schedule(() => {
      exitTimer = null;
      if (!destroyed && token === generation && !engaged(active)) close();
    }, exitGrace);
  };

  const activate = binding => {
    if (destroyed) return;
    binding.pointer ||= false;
    binding.focus ||= false;
    if (active !== binding) {
      clear('enter'); clear('exit');
      generation++;
      const hadActive = active !== null;
      active = binding;
      pointerInPreview = false;
      focusInPreview = false;
      if (hadActive) hide?.();
      const token = generation;
      enterTimer = schedule(async () => {
        enterTimer = null;
        if (destroyed || token !== generation || active !== binding || !engaged(binding)) return;
        try {
          const value = await load(binding.data);
          if (!destroyed && token === generation && active === binding && engaged(binding)) show?.(binding.data, value);
        } catch (error) {
          if (!destroyed && token === generation && active === binding && engaged(binding)) show?.(binding.data, null, error);
        }
      }, delay);
      return;
    }
    clear('exit');
  };

  const leavePreview = (mode, event) => {
    if (mode === 'pointer') pointerInPreview = false;
    else focusInPreview = false;
    const target = event?.relatedTarget;
    if (target && active?.anchor?.contains?.(target)) {
      if (mode === 'pointer') active.pointer = true;
      else active.focus = true;
      return;
    }
    if (!engaged(active)) scheduleHide();
  };

  function bind(target, previewTarget, data) {
    const binding = { anchor: target, data: { ...data, anchor: target }, pointer: false, focus: false };
    const localOff = [];
    localOff.push(listen(target, 'pointerenter', () => { binding.pointer = true; activate(binding); }));
    localOff.push(listen(target, 'pointerleave', event => {
      binding.pointer = false;
      if (event.relatedTarget && previewTarget?.contains?.(event.relatedTarget)) {
        pointerInPreview = true;
        clear('exit');
      } else if (active === binding && !engaged(binding)) scheduleHide();
    }));
    localOff.push(listen(target, 'focusin', () => { binding.focus = true; activate(binding); }));
    localOff.push(listen(target, 'focusout', event => {
      binding.focus = false;
      if (event.relatedTarget && previewTarget?.contains?.(event.relatedTarget)) {
        focusInPreview = true;
        clear('exit');
      } else if (active === binding && !engaged(binding)) scheduleHide();
    }));
    if (previewTarget && !boundPreviews.has(previewTarget)) {
      boundPreviews.add(previewTarget);
      listen(previewTarget, 'pointerenter', () => { pointerInPreview = true; clear('exit'); });
      listen(previewTarget, 'pointerleave', event => leavePreview('pointer', event));
      listen(previewTarget, 'focusin', () => { focusInPreview = true; clear('exit'); });
      listen(previewTarget, 'focusout', event => leavePreview('focus', event));
    }
    return () => {
      for (const off of localOff) off();
      binding.pointer = false; binding.focus = false;
      if (active === binding) close();
    };
  }

  return {
    bind,
    close,
    destroy() {
      destroyed = true;
      for (const off of [...unbind]) off();
      close();
    },
    scheduleHide,
  };
}
