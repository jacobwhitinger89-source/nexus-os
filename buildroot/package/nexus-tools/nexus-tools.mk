################################################################################
#
# nexus-tools
#
################################################################################

NEXUS_TOOLS_VERSION = local
NEXUS_TOOLS_SITE = $(BR2_EXTERNAL_NEXUS_PATH)/../out/rust-bin
NEXUS_TOOLS_SITE_METHOD = local
NEXUS_TOOLS_LICENSE = Apache-2.0

define NEXUS_TOOLS_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/nexus $(TARGET_DIR)/usr/bin/nexus
	$(INSTALL) -D -m 0755 $(@D)/nexusd $(TARGET_DIR)/usr/sbin/nexusd
	$(INSTALL) -D -m 0755 $(@D)/nexus-install $(TARGET_DIR)/usr/sbin/nexus-install
endef

$(eval $(generic-package))
