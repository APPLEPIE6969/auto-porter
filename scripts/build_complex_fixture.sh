#!/usr/bin/env bash
# Builds the COMPLEX fixture mod jar: tests/fixtures/jars/chunkcourier-neoforge-2.0.0.jar
#
# ChunkCourier deliberately spans every classification tier so the whole
# pipeline is exercised end-to-end (incl. the spec §10 LLM fallback):
#
#   Tier A  data content (recipe, loot table, advancement) + assets
#   Tier B  block-break + entity-interact listeners (templates),
#           generic mixin (@Inject HEAD), root config .toml
#   Tier B  player-tick listener -> player_tick_pre hook, NO template
#           => the LLM fallback path (porter.toml [llm])
#   Tier C  BlockEntity subclass, AbstractContainerMenu subclass,
#           CustomPayload networking class (templates, human design)
#   Tier D  RegisterCommandsEvent listener -> no Horizon hook in the
#           catalog => honest GapReport
#
# The stub APIs give the classes REAL RuntimeVisibleAnnotations and
# constant-pool refs; the jar carries only the mod's own classes.
set -euo pipefail

JDK=/home/z/my-project/.tools/jdk17/bin
ROOT=/home/z/my-project/auto-porter
FIX=$ROOT/tests/fixtures/jars
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# ---------------------------------------------------------------- stub APIs
mkdir -p "$WORK/stub/org/spongepowered/asm/mixin/injection"
mkdir -p "$WORK/stub/net/neoforged/fml/common"
mkdir -p "$WORK/stub/net/neoforged/bus/api"
mkdir -p "$WORK/stub/net/minecraftforge/event/entity/player"
mkdir -p "$WORK/stub/net/minecraftforge/event/entity/living"
mkdir -p "$WORK/stub/net/minecraftforge/event/level"
mkdir -p "$WORK/stub/net/minecraftforge/event"
mkdir -p "$WORK/stub/net/minecraft/commands"
mkdir -p "$WORK/stub/net/minecraft/world/entity/player"
mkdir -p "$WORK/stub/net/minecraft/world/inventory"
mkdir -p "$WORK/stub/net/minecraft/world/level/block/entity"
mkdir -p "$WORK/stub/net/minecraft/server/level"

cat > "$WORK/stub/org/spongepowered/asm/mixin/Mixin.java" <<'EOF'
package org.spongepowered.asm.mixin;
public @interface Mixin { Class<?>[] value() default {}; String[] targets() default {}; }
EOF
cat > "$WORK/stub/org/spongepowered/asm/mixin/injection/Inject.java" <<'EOF'
package org.spongepowered.asm.mixin.injection;
import org.spongepowered.asm.mixin.injection.At;
public @interface Inject {
    String[] method() default {};
    At at();
    boolean cancellable() default false;
}
EOF
cat > "$WORK/stub/org/spongepowered/asm/mixin/injection/At.java" <<'EOF'
package org.spongepowered.asm.mixin.injection;
public @interface At { String value(); }
EOF
cat > "$WORK/stub/net/neoforged/fml/common/Mod.java" <<'EOF'
package net.neoforged.fml.common;
public @interface Mod { String value(); }
EOF
cat > "$WORK/stub/net/neoforged/bus/api/SubscribeEvent.java" <<'EOF'
package net.neoforged.bus.api;
public @interface SubscribeEvent {}
EOF
cat > "$WORK/stub/net/minecraftforge/event/level/BlockEvent.java" <<'EOF'
package net.minecraftforge.event.level;
public class BlockEvent {
    public static class BreakEvent {
        public void setCanceled(boolean c) {}
    }
}
EOF
cat > "$WORK/stub/net/minecraftforge/event/entity/player/PlayerInteractEvent.java" <<'EOF'
package net.minecraftforge.event.entity.player;
public class PlayerInteractEvent {
    public static class EntityInteract {
        public void setCanceled(boolean c) {}
    }
}
EOF
cat > "$WORK/stub/net/minecraftforge/event/TickEvent.java" <<'EOF'
package net.minecraftforge.event;
import net.minecraft.world.entity.player.PlayerEntity;
public class TickEvent {
    public static class PlayerTickEvent {
        public PlayerEntity player;
        public int phase;
    }
}
EOF
cat > "$WORK/stub/net/minecraftforge/event/RegisterCommandsEvent.java" <<'EOF'
package net.minecraftforge.event;
import net.minecraft.commands.CommandDispatcher;
public class RegisterCommandsEvent {
    public CommandDispatcher getDispatcher() { return new CommandDispatcher(); }
}
EOF
cat > "$WORK/stub/net/minecraft/commands/CommandDispatcher.java" <<'EOF'
package net.minecraft.commands;
public class CommandDispatcher {
    public void register(Object command) {}
}
EOF
cat > "$WORK/stub/net/minecraft/world/entity/player/PlayerEntity.java" <<'EOF'
package net.minecraft.world.entity.player;
public class PlayerEntity {
    public void sendSystemMessage(String message) {}
}
EOF
cat > "$WORK/stub/net/minecraft/world/inventory/AbstractContainerMenu.java" <<'EOF'
package net.minecraft.world.inventory;
import net.minecraft.world.entity.player.PlayerEntity;
public class AbstractContainerMenu {
    public boolean stillValid(PlayerEntity player) { return true; }
}
EOF
cat > "$WORK/stub/net/minecraft/world/level/block/entity/BlockEntity.java" <<'EOF'
package net.minecraft.world.level.block.entity;
public class BlockEntity {
    public void setChanged() {}
}
EOF
cat > "$WORK/stub/net/minecraft/server/level/ServerChunkCache.java" <<'EOF'
package net.minecraft.server.level;
public class ServerChunkCache {
    public void tickChunks() {}
}
EOF

mkdir -p "$WORK/stub-classes"
find "$WORK/stub" -name '*.java' > "$WORK/stub-sources.txt"
"$JDK/javac" --release 8 -nowarn -d "$WORK/stub-classes" @"$WORK/stub-sources.txt"

# ---------------------------------------------------------------- the mod
C="$WORK/chunkcourier"
mkdir -p "$C/src/com/example/chunkcourier/blockentity" \
         "$C/src/com/example/chunkcourier/menu" \
         "$C/src/com/example/chunkcourier/net" \
         "$C/src/com/example/chunkcourier/mixin" \
         "$C/res/META-INF" \
         "$C/res/assets/chunkcourier/lang" \
         "$C/res/assets/chunkcourier/textures/block" \
         "$C/res/assets/chunkcourier/models/block" \
         "$C/res/data/chunkcourier/recipe" \
         "$C/res/data/chunkcourier/loot_table/blocks" \
         "$C/res/data/chunkcourier/advancement"

cat > "$C/res/META-INF/neoforge.mods.toml" <<'EOF'
modLoader = "javafml"
loaderVersion = "[1,)"
license = "MIT"

[[mods]]
modId = "chunkcourier"
version = "2.0.0"
displayName = "ChunkCourier"
description = "Chunk-load parcels, courier pads and remote menus."
authors = "fixture"

[[dependencies.chunkcourier]]
modId = "minecraft"
versionRange = "[1.21,)"
ordering = "NONE"
side = "BOTH"
EOF

cat > "$C/res/chunkcourier.toml" <<'EOF'
parcelsPerChunk = 4
allowRemoteMenus = true
courierFee = 3
EOF

cat > "$C/src/com/example/chunkcourier/ChunkCourier.java" <<'EOF'
package com.example.chunkcourier;

import net.minecraftforge.event.RegisterCommandsEvent;
import net.minecraftforge.event.TickEvent;
import net.minecraftforge.event.entity.player.PlayerInteractEvent;
import net.minecraftforge.event.level.BlockEvent;
import net.neoforged.bus.api.SubscribeEvent;
import net.neoforged.fml.common.Mod;

@Mod("chunkcourier")
public class ChunkCourier {
    private int ticks;

    @SubscribeEvent
    public void onBreak(BlockEvent.BreakEvent event) {
        event.setCanceled(false);
    }

    @SubscribeEvent
    public void onEntityInteract(PlayerInteractEvent.EntityInteract event) {
        event.setCanceled(false);
    }

    @SubscribeEvent
    public void onPlayerTick(TickEvent.PlayerTickEvent event) {
        if (event.player == null) {
            return;
        }
        ticks++;
        if (ticks % 20 == 0) {
            event.player.sendSystemMessage("chunkcourier: parcels ticked " + ticks);
        }
    }

    @SubscribeEvent
    public void onRegisterCommands(RegisterCommandsEvent event) {
        event.getDispatcher().register(new CourierCommand());
    }

    public static class CourierCommand {
    }
}
EOF

cat > "$C/src/com/example/chunkcourier/blockentity/ParcelBlockEntity.java" <<'EOF'
package com.example.chunkcourier.blockentity;

import net.minecraft.world.level.block.entity.BlockEntity;

public class ParcelBlockEntity extends BlockEntity {
    private int contents;

    public void tick() {
        if (contents > 0) {
            contents--;
            setChanged();
        }
    }
}
EOF

cat > "$C/src/com/example/chunkcourier/menu/ParcelMenu.java" <<'EOF'
package com.example.chunkcourier.menu;

import net.minecraft.world.entity.player.PlayerEntity;
import net.minecraft.world.inventory.AbstractContainerMenu;

public class ParcelMenu extends AbstractContainerMenu {
    @Override
    public boolean stillValid(PlayerEntity player) {
        return true;
    }
}
EOF

cat > "$C/src/com/example/chunkcourier/net/CourierCustomPayload.java" <<'EOF'
package com.example.chunkcourier.net;

public class CourierCustomPayload {
    public int parcelX;
    public int parcelY;
    public int parcelZ;
}
EOF

cat > "$C/src/com/example/chunkcourier/mixin/MixinServerChunkCache.java" <<'EOF'
package com.example.chunkcourier.mixin;

import net.minecraft.server.level.ServerChunkCache;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;

@Mixin(ServerChunkCache.class)
public class MixinServerChunkCache {
    @Inject(method = "tickChunks", at = @At("HEAD"), cancellable = true)
    private void chunkcourier$beforeTickChunks() {
    }
}
EOF

printf 'PNG' > "$C/res/assets/chunkcourier/textures/block/parcel_block.png"
cat > "$C/res/assets/chunkcourier/lang/en_us.json" <<'EOF'
{ "block.chunkcourier.parcel_block": "Parcel Block" }
EOF
cat > "$C/res/assets/chunkcourier/models/block/parcel_block.json" <<'EOF'
{
  "parent": "minecraft:block/cube_all",
  "textures": { "all": "chunkcourier:block/parcel_block" }
}
EOF
cat > "$C/res/data/chunkcourier/recipe/parcel_block.json" <<'EOF'
{
  "type": "minecraft:crafting_shaped",
  "pattern": ["III", "IC I", "III"],
  "key": { "I": { "item": "minecraft:iron_ingot" }, "C": { "item": "minecraft:chest" } },
  "result": { "item": "chunkcourier:parcel_block", "count": 1 }
}
EOF
cat > "$C/res/data/chunkcourier/loot_table/blocks/parcel_block.json" <<'EOF'
{
  "type": "minecraft:block",
  "pools": [{ "rolls": 1, "entries": [{ "type": "minecraft:item", "name": "chunkcourier:parcel_block" }] }]
}
EOF
cat > "$C/res/data/chunkcourier/advancement/root.json" <<'EOF'
{
  "display": { "icon": { "item": "chunkcourier:parcel_block" }, "title": "ChunkCourier" },
  "criteria": { "tick": { "trigger": "minecraft:tick" } }
}
EOF

mkdir -p "$WORK/classes"
find "$C/src" -name '*.java' > "$WORK/sources.txt"
"$JDK/javac" --release 8 -nowarn -cp "$WORK/stub-classes" -d "$WORK/classes" @"$WORK/sources.txt"
cp -r "$C/res/." "$WORK/classes/"
mkdir -p "$FIX"
(cd "$WORK/classes" && "$JDK/jar" cf "$FIX/chunkcourier-neoforge-2.0.0.jar" .)
echo "built $FIX/chunkcourier-neoforge-2.0.0.jar"
