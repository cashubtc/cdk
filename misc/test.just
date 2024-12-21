itest db:
  #!/usr/bin/env bash
  ./misc/itests.sh "{{db}}"

  
fake-mint-itest db:
  #!/usr/bin/env bash
  ./misc/fake_itests.sh "{{db}}"

  
fake-auth-mint-itest db openid_discovery:
  #!/usr/bin/env bash
  ./misc/fake_auth_itests.sh "{{db}}" "{{openid_discovery}}"

