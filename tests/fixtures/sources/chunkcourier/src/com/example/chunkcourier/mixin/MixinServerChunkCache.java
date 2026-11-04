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
