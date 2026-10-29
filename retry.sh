#!/bin/sh
for delay in 60 120 240 240; do
  date -u
  gh api --include rate_limit
  gh api --method PUT repos/iarnvidr/chat-hub/environments/chat-hub-release -f prevent_self_review=true -f 'deployment_branch_policy[protected_branches]=false' -f 'deployment_branch_policy[custom_branch_policies]=true' && exit 0
  sleep "$delay"
done
exit 1
