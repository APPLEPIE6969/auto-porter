#!/usr/bin/env bash
# Builds the three fixture mod JARs in tests/fixtures/jars/ using the portable
# JDK. The annotation/stub sources give the fixture classes REAL
# RuntimeVisibleAnnotations / constant-pool refs, exactly what the decompose
# phase parses.
set -euo pipefail

JDK=/home/z/my-project/.tools/jdk17/bin
ROOT=/home/z/my-project/auto-porter
FIX=$ROOT/tests/fixtures/jars
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$FIX"

# ---------------------------------------------------------------- stub APIs
mkdir -p "$WORK/stub/org/spongepowered/asm/mixin/injection"
mkdir -p "$WORK/stub/org/spongepowered/asm/mixin"
mkdir -p "$WORK/stub/net/fabricmc/api"
mkdir -p "$WORK/stub/net/fabricmc/fabric/api/event/player"
mkdir -p "$WORK/stub/net/minecraft/block"
mkdir -p "$WORK/stub/net/minecraft/world/level/block/entity"
mkdir -p "$WORK/stub/net/minecraft/client/renderer"
mkdir -p "$WORK/stub/net/neoforged/fml/common"
mkdir -p "$WORK/stub/net/neoforged/bus/api"
mkdir -p "$WORK/stub/net/minecraftforge/event/level"
mkdir -p "$WORK/stub/net/minecraftforge/event/entity/living"
mkdir -p "$WORK/stub/net/minecraft/world/entity/player"

cat > "$WORK/stub/org/spongepowered/asm/mixin/Mixin.java" <<'EOF'
package org.spongepowered.asm.mixin;
public @interface Mixin { Class<?>[] value() default {}; String[] targets() default {}; }
EOF
cat > "$WORK/stub/org/spongepowered/asm/mixin/Overwrite.java" <<'EOF'
package org.spongepowered.asm.mixin;
public @interface Overwrite {}
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

cat > "$WORK/stub/net/fabricmc/api/ModInitializer.java" <<'EOF'
package net.fabricmc.api;
public interface ModInitializer { void onInitialize(); }
EOF
cat > "$WORK/stub/net/fabricmc/fabric/api/event/player/PlayerBlockBreakEvents.java" <<'EOF'
package net.fabricmc.fabric.api.event.player;
public final class PlayerBlockBreakEvents {
    private PlayerBlockBreakEvents() {}
    public interface Event<T> {
        default void register(T listener) {}
    }
    public interface Before extends Event<Before> {
        Before EVENT = new Before() {
            public void register(Before listener) {}
            public boolean beforeBlockBreak(net.minecraft.world.entity.player.PlayerEntity player,
                                            net.minecraft.block.Block block) { return true; }
        };
        boolean beforeBlockBreak(net.minecraft.world.entity.player.PlayerEntity player,
                                 net.minecraft.block.Block block);
    }
}
EOF
cat > "$WORK/stub/net/minecraft/block/Block.java" <<'EOF'
package net.minecraft.block;
public class Block {
    public void playerDestroy() {}
}
EOF
cat > "$WORK/stub/net/minecraft/world/entity/player/PlayerEntity.java" <<'EOF'
package net.minecraft.world.entity.player;
public class PlayerEntity {}
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
    public static class EntityPlaceEvent {
        public void setCanceled(boolean c) {}
    }
}
EOF
cat > "$WORK/stub/net/minecraftforge/event/entity/living/LivingHurtEvent.java" <<'EOF'
package net.minecraftforge.event.entity.living;
public class LivingHurtEvent {
    public float getAmount() { return 0f; }
    public void setAmount(float a) {}
}
EOF
cat > "$WORK/stub/net/minecraft/world/level/block/entity/BlockEntity.java" <<'EOF'
package net.minecraft.world.level.block.entity;
public class BlockEntity {
    public void setChanged() {}
}
EOF
cat > "$WORK/stub/net/minecraft/client/renderer/PostChain.java" <<'EOF'
package net.minecraft.client.renderer;
public class PostChain {}
EOF

# compile the stub APIs once; fixture sources see them via -classpath so the
# jars carry ONLY the mod's own classes (like a real mod jar)
mkdir -p "$WORK/stub-classes"
find "$WORK/stub" -name '*.java' > "$WORK/stub-sources.txt"
"$JDK/javac" --release 8 -nowarn -d "$WORK/stub-classes" @"$WORK/stub-sources.txt"

compile_and_jar() { # $1 = src dir, $2 = jar path, $3 = extra resource dir
    local SRC=$1 OUT=$2 RES=$3
    local NAME
    NAME=$(basename "$OUT")
    mkdir -p "$WORK/classes-$NAME"
    find "$SRC" -name '*.java' > "$WORK/sources-$NAME.txt"
    "$JDK/javac" --release 8 -nowarn -cp "$WORK/stub-classes" -d "$WORK/classes-$NAME" @"$WORK/sources-$NAME.txt"
    if [ -n "$RES" ]; then cp -r "$RES/." "$WORK/classes-$NAME/"; fi
    (cd "$WORK/classes-$NAME" && "$JDK/jar" cf "$OUT" .)
}

# ------------------------------------------------- fixture 1: veinminer (Fabric)
V="$WORK/veinminer"
mkdir -p "$V/src/com/example/veinminer/mixin" "$V/res/assets/veinminer/lang" \
         "$V/res/assets/veinminer/textures/item" "$V/res/assets/veinminer/models/item" \
         "$V/res/data/veinminer/recipe" "$V/res/data/veinminer/loot_table/blocks"

cat > "$V/res/fabric.mod.json" <<'EOF'
{
  "schemaVersion": 1,
  "id": "veinminer",
  "version": "1.0.0",
  "name": "VeinMiner",
  "description": "Break a vein of ore at once.",
  "authors": ["fixture"],
  "license": "MIT",
  "environment": "*",
  "entrypoints": { "main": ["com.example.veinminer.VeinMiner"] },
  "mixins": ["veinminer.mixins.json"],
  "depends": { "fabricloader": ">=0.15.0", "minecraft": ">=1.21" }
}
EOF
cat > "$V/res/veinminer.mixins.json" <<'EOF'
{
  "required": true,
  "package": "com.example.veinminer.mixin",
  "compatibilityLevel": "JAVA_8",
  "mixins": ["MixinBlock"],
  "injectors": { "defaultRequire": 1 }
}
EOF
cat > "$V/res/veinminer.toml" <<'EOF'
maxVeinSize = 16
requiresSneaking = true
EOF
cat > "$V/src/com/example/veinminer/VeinMiner.java" <<'EOF'
package com.example.veinminer;

import net.fabricmc.api.ModInitializer;
import net.fabricmc.fabric.api.event.player.PlayerBlockBreakEvents;

public class VeinMiner implements ModInitializer {
    @Override
    public void onInitialize() {
        PlayerBlockBreakEvents.Before.EVENT.register(new VeinMinerHandler());
    }
}
EOF
cat > "$V/src/com/example/veinminer/VeinMinerHandler.java" <<'EOF'
package com.example.veinminer;

import net.fabricmc.api.ModInitializer;
import net.fabricmc.fabric.api.event.player.PlayerBlockBreakEvents;

public class VeinMinerHandler implements PlayerBlockBreakEvents.Before {
    @Override
    public boolean beforeBlockBreak(net.minecraft.world.entity.player.PlayerEntity player,
                                    net.minecraft.block.Block block) {
        return true;
    }
}
EOF
cat > "$V/src/com/example/veinminer/mixin/MixinBlock.java" <<'EOF'
package com.example.veinminer.mixin;

import net.minecraft.block.Block;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;

@Mixin(Block.class)
public class MixinBlock {
    @Inject(method = "playerDestroy", at = @At("HEAD"), cancellable = true)
    private void veinminer$onBreak() {
    }
}
EOF
printf 'PNG' > "$V/res/assets/veinminer/textures/item/vein_pickaxe.png"
cat > "$V/res/assets/veinminer/lang/en_us.json" <<'EOF'
{ "item.veinminer.vein_pickaxe": "Vein Pickaxe" }
EOF
cat > "$V/res/assets/veinminer/models/item/vein_pickaxe.json" <<'EOF'
{
  "parent": "minecraft:item/generated",
  "textures": { "layer0": "veinminer:item/vein_pickaxe" }
}
EOF
cat > "$V/res/data/veinminer/recipe/vein_pickaxe.json" <<'EOF'
{
  "type": "minecraft:crafting_shaped",
  "pattern": ["III", " S ", " S "],
  "key": { "I": { "item": "minecraft:iron_ingot" }, "S": { "item": "minecraft:stick" } },
  "result": { "item": "veinminer:vein_pickaxe", "count": 1 }
}
EOF
cat > "$V/res/data/veinminer/loot_table/blocks/vein_ore.json" <<'EOF'
{
  "type": "minecraft:block",
  "pools": [{ "rolls": 1, "entries": [{ "type": "minecraft:item", "name": "minecraft:raw_iron" }] }]
}
EOF

compile_and_jar "$V/src" "$FIX/veinminer-fabric-1.0.0.jar" "$V/res"
echo "built $FIX/veinminer-fabric-1.0.0.jar"

# ------------------------------------------------ fixture 2: torchtools (NeoForge)
T="$WORK/torchtools"
mkdir -p "$T/src/com/example/torchtools/blockentity" "$T/src/com/example/torchtools/client" \
         "$T/res/META-INF" "$T/res/assets/torchtools/lang" \
         "$T/res/assets/torchtools/textures/block" "$T/res/assets/torchtools/models/block" \
         "$T/res/data/torchtools/recipe"

cat > "$T/res/META-INF/neoforge.mods.toml" <<'EOF'
modLoader = "javafml"
loaderVersion = "[1,)"
license = "MIT"

[[mods]]
modId = "torchtools"
version = "0.3.1"
displayName = "TorchTools"
description = "Charged lamps and torch utilities."
authors = "fixture"

[[dependencies.torchtools]]
modId = "minecraft"
versionRange = "[1.21,)"
ordering = "NONE"
side = "BOTH"
EOF
cat > "$T/res/torchtools.toml" <<'EOF'
lampRange = 12
energyPerTick = 40
EOF
cat > "$T/src/com/example/torchtools/TorchTools.java" <<'EOF'
package com.example.torchtools;

import net.minecraftforge.event.entity.living.LivingHurtEvent;
import net.minecraftforge.event.level.BlockEvent;
import net.neoforged.bus.api.SubscribeEvent;
import net.neoforged.fml.common.Mod;

@Mod("torchtools")
public class TorchTools {
    @SubscribeEvent
    public void onBreak(BlockEvent.BreakEvent event) {
        event.setCanceled(false);
    }

    @SubscribeEvent
    public void onHurt(LivingHurtEvent event) {
        event.setAmount(event.getAmount());
    }
}
EOF
cat > "$T/src/com/example/torchtools/blockentity/ChargedLampBlockEntity.java" <<'EOF'
package com.example.torchtools.blockentity;

import net.minecraft.world.level.block.entity.BlockEntity;

public class ChargedLampBlockEntity extends BlockEntity {
    private int energy;

    public void tick() {
        energy = Math.max(0, energy - 1);
        setChanged();
    }
}
EOF
cat > "$T/src/com/example/torchtools/client/ChargedLampShader.java" <<'EOF'
package com.example.torchtools.client;

import net.minecraft.client.renderer.PostChain;

public class ChargedLampShader extends PostChain {
}
EOF
printf 'PNG' > "$T/res/assets/torchtools/textures/block/charged_lamp.png"
cat > "$T/res/assets/torchtools/models/block/charged_lamp.json" <<'EOF'
{
  "parent": "minecraft:block/cube_all",
  "textures": { "all": "torchtools:block/charged_lamp" }
}
EOF
cat > "$T/res/assets/torchtools/lang/en_us.json" <<'EOF'
{ "block.torchtools.charged_lamp": "Charged Lamp" }
EOF
cat > "$T/res/data/torchtools/recipe/charged_lamp.json" <<'EOF'
{
  "type": "minecraft:crafting_shaped",
  "pattern": ["GTG", "TCT", "GTG"],
  "key": { "G": { "item": "minecraft:glass" }, "T": { "item": "minecraft:torch" }, "C": { "item": "minecraft:copper_ingot" } },
  "result": { "item": "torchtools:charged_lamp", "count": 1 }
}
EOF

compile_and_jar "$T/src" "$FIX/torchtools-neoforge-0.3.1.jar" "$T/res"
echo "built $FIX/torchtools-neoforge-0.3.1.jar"

# ------------------------------------------------ fixture 3: moreassets (Fabric, data only)
M="$WORK/moreassets"
mkdir -p "$M/res/assets/moreassets/lang" "$M/res/assets/moreassets/textures/item" \
         "$M/res/assets/moreassets/models/item" "$M/res/data/moreassets/recipe"

cat > "$M/res/fabric.mod.json" <<'EOF'
{
  "schemaVersion": 1,
  "id": "moreassets",
  "version": "2.1.0",
  "name": "MoreAssets",
  "description": "Pure content: textures, models, recipes.",
  "authors": ["fixture"],
  "license": "MIT",
  "environment": "*",
  "depends": { "minecraft": ">=1.21" }
}
EOF
cat > "$M/res/assets/moreassets/lang/en_us.json" <<'EOF'
{ "item.moreassets.shiny_gear": "Shiny Gear" }
EOF
printf 'PNG' > "$M/res/assets/moreassets/textures/item/shiny_gear.png"
cat > "$M/res/assets/moreassets/models/item/shiny_gear.json" <<'EOF'
{
  "parent": "minecraft:item/generated",
  "textures": { "layer0": "moreassets:item/shiny_gear" }
}
EOF
cat > "$M/res/data/moreassets/recipe/shiny_gear.json" <<'EOF'
{
  "type": "minecraft:crafting_shapeless",
  "ingredients": [{ "item": "minecraft:iron_ingot" }, { "item": "minecraft:gold_ingot" }],
  "result": { "item": "moreassets:shiny_gear", "count": 1 }
}
EOF

# no sources: jar directly from res
(cd "$M/res" && "$JDK/jar" cf "$FIX/moreassets-fabric-2.1.0.jar" .)
echo "built $FIX/moreassets-fabric-2.1.0.jar"

ls -la "$FIX"
